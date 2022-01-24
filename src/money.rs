use num_rational::Rational32;
use num_traits::{Zero, ToPrimitive};

use std::fmt;
use std::cmp;

use crate::config::CONFIG;

// https://wiki.guildwars2.com/wiki/Trading_Post
// Listing Fee (5%) — This nonrefundable cost covers listing and holding your items for sale. This
// fee has a minimum of 1c and is immediately taken from your wallet when you list or instantly
// sell an item.
// Exchange Fee (10%) — This fee is the Trading Post's cut of the profit. This fee has a minimum of
// 1c and is deducted from coins delivered to the seller after a successful sale.
const TRADING_POST_LISTING_FEE: u8 = 5; // %
const TRADING_POST_EXCHANGE_FEE: u8 = 10; // %

// TODO: spirit shards, laurels,
// badges of honor? Testimony/proof of heroics
// Geodes, Bandit Crests, Airship Parts, Aurillium, Ley Crystals, Trade Contracts, Racing Medallions
// Fractal Relics
#[derive(Debug, Copy, Clone, Eq)]
pub struct Money {
    copper: Rational32,
    karma: Rational32,
    um: Rational32,
    vm: Rational32,
}
impl Money {
    pub fn from_copper(copper: i32) -> Self {
        Self {
            copper: Rational32::from(copper),
            ..Default::default()
        }
    }
    pub fn from_um(um: i32) -> Self {
        Self {
            um: Rational32::from(um),
            ..Default::default()
        }
    }
    pub fn from_vm(vm: i32) -> Self {
        Self {
            vm: Rational32::from(vm),
            ..Default::default()
        }
    }
    pub fn new(copper: i32, karma: i32, um: i32, vm: i32) -> Self {
        Self {
            copper: Rational32::from(copper),
            karma: Rational32::from(karma),
            um: Rational32::from(um),
            vm: Rational32::from(vm),
        }
    }

    fn copper_value(&self) -> Rational32 {
        self.copper
         + self.karma * CONFIG.karma.unwrap_or(Rational32::zero())
         + self.um * CONFIG.um.unwrap_or(Rational32::zero())
         + self.vm * CONFIG.vm.unwrap_or(Rational32::zero())
    }
    pub fn to_copper_value(&self) -> i32 {
        self.copper_value().ceil().to_integer()
    }

    fn fee(&self, percent: u8) -> Rational32 {
        cmp::max(Rational32::from(1), self.copper * Rational32::new(percent as i32, 100)).round()
    }

    pub fn trading_post_sale_revenue(self) -> Money {
        let fees = self.fee(TRADING_POST_EXCHANGE_FEE) + self.fee(TRADING_POST_LISTING_FEE);
        Money {
            copper: if self.copper > fees { self.copper - fees } else { 0.into() },
            ..Default::default()
        }
    }
    /// Has an error of at most 1 copper too high (could have broken even at one copper less)
    pub fn trading_post_listing_price(self) -> Money {
        let copper = self.copper_value();
        Money {
            copper: cmp::max(
                (copper * Rational32::new(
                    100, (100 - TRADING_POST_LISTING_FEE - TRADING_POST_EXCHANGE_FEE) as i32
                )).ceil(),
                copper + 2,
            ),
            ..Default::default()
        }
    }
    /// This will probably not be precise due to rounding errors accumulating as sales are broken
    /// into smaller batches. Would need to calculate this per sale chunk to be precise - but then
    /// the TP sometimes glitches w/small sale volumes, requiring filling them multiple times
    /// anyway, reintroducing rounding errors.
    pub fn increase_by_listing_fee(self) -> Money {
        Money {
            copper: self.copper + self.fee(TRADING_POST_LISTING_FEE),
            karma: self.karma,
            um: self.um,
            vm: self.vm,
        }
    }

    // integer division rounding up
    // see: https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
    pub fn div_i32_ceil(self, y: i32) -> Money {
        Money {
            copper: (self.copper + y - 1) / y,
            karma: (self.karma + y - 1) / y,
            um: (self.um + y - 1) / y,
            vm: (self.vm + y - 1) / y,
        }
    }

    // Gives an approximate ratio between two money values; for profit on cost
    pub fn percent(self, other: Self) -> f64 {
        let value = self.copper_value().to_f64().unwrap_or(0_f64);
        let other_value = other.copper_value().to_f64().unwrap_or(f64::INFINITY);
        value / other_value
    }
}
impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let copper = self.copper.to_integer();
        let sign = if copper < 0 { "-" } else { "" };
        let copper = copper.abs();

        let gold = copper / 10000;
        let silver = (copper - gold * 10000) / 100;
        let copper = copper - gold * 10000 - silver * 100;

        let mut display = format!("{}{}.{:02}.{:02}g", sign, gold, silver, copper);

        if self.karma != Rational32::zero() {
            display = format!("{}, {} Karma", display, self.karma.to_integer());
        }

        if self.um != Rational32::zero() {
            display = format!("{}, {} UM", display, self.um.to_integer());
        }

        if self.vm != Rational32::zero() {
            display = format!("{}, {} VM", display, self.vm.to_integer());
        }

        write!(f, "{}", display)
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
            copper: Rational32::zero(),
            karma: Rational32::zero(),
            um: Rational32::zero(),
            vm: Rational32::zero(),
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
            karma: self.karma + other.karma,
            um: self.um + other.um,
            vm: self.vm + other.vm,
        }
    }
}
impl std::ops::Sub for Money {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            copper: self.copper - other.copper,
            karma: self.karma - other.karma,
            um: self.um - other.um,
            vm: self.vm - other.vm,
        }
    }
}
impl std::ops::AddAssign for Money {
    fn add_assign(&mut self, other: Self) {
        *self = Self {
            copper: self.copper + other.copper,
            karma: self.karma + other.karma,
            um: self.um + other.um,
            vm: self.vm + other.vm,
        }
    }
}
impl std::ops::Mul<u32> for Money {
    type Output = Self;

    fn mul(self, other: u32) -> Self {
        Self {
            copper: self.copper * other as i32,
            karma: self.karma * other as i32,
            um: self.um * other as i32,
            vm: self.vm * other as i32,
        }
    }
}
impl std::ops::Div<u32> for Money {
    type Output = Self;

    fn div(self, other: u32) -> Self {
        Self {
            copper: self.copper / other as i32,
            karma: self.karma / other as i32,
            um: self.um / other as i32,
            vm: self.vm / other as i32,
        }
    }
}
impl PartialEq for Money {
    fn eq(&self, other: &Self) -> bool {
        self.copper == other.copper
            && self.karma == other.karma
            && self.um == other.um
            && self.vm == other.vm
    }
}
impl PartialOrd for Money {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Money {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let value = self.copper_value();
        let other_value = other.copper_value();
        value.cmp(&other_value)
    }
}
impl std::iter::Sum for Money {
    fn sum<I: Iterator<Item = Money>>(iter: I) -> Self {
        let mut sink = Money::zero();
        for src in iter {
            sink.copper += src.copper;
            sink.karma += src.karma;
            sink.um += src.um;
            sink.vm += src.vm;
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
