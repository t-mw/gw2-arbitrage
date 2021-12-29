use num_rational::Ratio;
use num_traits::{Zero, ToPrimitive};

use std::fmt;
use std::cmp;

// https://wiki.guildwars2.com/wiki/Trading_Post
// Listing Fee (5%) — This nonrefundable cost covers listing and holding your items for sale. This
// fee has a minimum of 1c and is immediately taken from your wallet when you list or instantly
// sell an item.
// Exchange Fee (10%) — This fee is the Trading Post's cut of the profit. This fee has a minimum of
// 1c and is deducted from coins delivered to the seller after a successful sale.
const TRADING_POST_LISTING_FEE: u32 = 5; // %
const TRADING_POST_EXCHANGE_FEE: u32 = 10; // %

type Rational32u = Ratio<u32>;

#[derive(Debug, Copy, Clone, Eq)]
pub struct Money {
    copper: Rational32u,
    // karma: Rational32u,
}
impl Money {
    pub fn from_copper(copper: u32) -> Self {
        Self {
            copper: Rational32u::from(copper),
        }
    }
    pub fn to_copper_value(self) -> u32 {
        self.copper.ceil().to_integer()
    }

    fn fee(&self, percent: u32) -> Rational32u {
        cmp::max(Rational32u::from(1), self.copper * Rational32u::new(percent, 100)).round()
    }

    pub fn trading_post_sale_revenue(self) -> Money {
        let fees = self.fee(TRADING_POST_EXCHANGE_FEE) + self.fee(TRADING_POST_LISTING_FEE);
        Money {
            copper: if self.copper > fees { self.copper - fees } else { 0.into() },
        }
    }
    /// Has an error of at most 1 copper too high (could have broken even at one copper less)
    pub fn trading_post_listing_price(self) -> Money {
        Money {
            copper: cmp::max(
                (self.copper * Rational32u::new(100, 100 - TRADING_POST_LISTING_FEE - TRADING_POST_EXCHANGE_FEE)).ceil(),
                self.copper + 2,
            ),
        }
    }
    /// This will probably not be precise due to rounding errors accumulating as sales are broken
    /// into smaller batches. Would need to calculate this per sale chunk to be precise - but then
    /// the TP sometimes glitches w/small sale volumes, requiring filling them multiple times
    /// anyway, reintroducing rounding errors.
    pub fn increase_by_listing_fee(self) -> Money {
        Money {
            copper: self.copper + self.fee(TRADING_POST_LISTING_FEE),
        }
    }

    // integer division rounding up
    // see: https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
    pub fn div_u32_ceil(self, y: u32) -> Money {
        Money {
            copper: (self.copper + y - 1) / y,
        }
    }

    // Gives an approximate ratio between two money values; for profit on cost
    pub fn percent(self, other: Self) -> f64 {
        self.copper.to_f64().unwrap() / other.copper.to_f64().unwrap()
    }

    fn copper_to_string(copper: u32) -> String {
        let gold = copper / 10000;
        let silver = (copper - gold * 10000) / 100;
        let copper = copper - gold * 10000 - silver * 100;
        format!("{}.{:02}.{:02}g", gold, silver, copper)
    }
}
impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", Money::copper_to_string(self.copper.to_integer()))
    }
}
impl Default for Money {
    fn default() -> Self {
        Self::zero()
    }
}
impl Zero for Money {
    fn zero() -> Self {
        Self {
            copper: Rational32u::zero(),
        }
    }
    fn is_zero(&self) -> bool {
        self.copper.is_zero()
    }
}
impl std::ops::Add for Money {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            copper: self.copper + other.copper,
        }
    }
}
impl std::ops::Sub for Money {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        debug_assert!(self.copper >= other.copper);
        Self {
            copper: self.copper - other.copper,
        }
    }
}
impl std::ops::AddAssign for Money {
    fn add_assign(&mut self, other: Self) {
        *self = Self {
            copper: self.copper + other.copper,
        }
    }
}
impl std::ops::Mul<u32> for Money {
    type Output = Self;

    fn mul(self, other: u32) -> Self {
        Self {
            copper: self.copper * other,
        }
    }
}
impl std::ops::Div<u32> for Money {
    type Output = Self;

    fn div(self, other: u32) -> Self {
        Self {
            copper: self.copper / other,
        }
    }
}
impl PartialEq for Money {
    fn eq(&self, other: &Self) -> bool {
        self.copper == other.copper
    }
}
impl PartialOrd for Money {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Money {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.copper.cmp(&other.copper)
    }
}
impl std::iter::Sum for Money {
    fn sum<I: Iterator<Item = Money>>(iter: I) -> Self {
        let mut sink = Money::zero();
        for src in iter {
            sink.copper += src.copper;
        }
        sink
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sale_revenue() {
        // NOTE: all prices verified in game; game subtracts listing fee from
        // cash and exchange fee from revenue.
        let prices = vec![
            (2, 0),
            (6, 4), (6 * 2, 10), (6 * 3, 15),
            (51, 43),
            (68, 58),
        ];
        for (sell, revenue) in prices {
            assert_eq!(
                revenue,
                Money::from_copper(sell)
                    .trading_post_sale_revenue()
                    .to_copper_value()
            );
        }
    }

    #[test]
    fn listing_price() {
        let epsilon = Money::from_copper(1);
        // Bunch of arbitrary primes
        let values = vec![
            1, 2, 3, 17, 31, 37, 47, 53, 71, 101, 137,
            3499, 9431,
            100673, 199799,
            1385507,
            24710753,
        ];
        for value in values {
            let price = Money::from_copper(value);
            let breakeven = Money::from_copper(
                price.trading_post_listing_price().to_copper_value()
            ).trading_post_sale_revenue();
            assert!(price <= breakeven && breakeven <= price + epsilon);
        }
    }
}
