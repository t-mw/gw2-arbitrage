# gw2-arbitrage

Finds items in Guild Wars 2 that can be sold on the trading post for a higher price than the cost of crafting the item.

## Usage

Run the binary with `cargo run --release` to produce a list of items that can be crafted and immediately resold
for profit on the trading post using ingredients purchased from the trading post. Items with high 'profit / step'
and 'profit on cost' values generally produce a higher return for the time invested.

![List of items](screen1.png)

Pass an item id as input (e.g. `cargo run --release -- 11538`) to print a shopping list for the item, which considers
the total available liquidity for each ingredient on the trading post. By default the shopping list will assume that you
want to produce as many copies of the item as can be profitably sold on the trading post. To limit the number of items
that will be crafted a count may also be passed (e.g. `cargo run --release -- 11538 100` will limit the shopping list to producing 100 items).

![List of ingredients](screen2.png)

Detailed crafting instructions for the item can then be found on https://www.gw2bltc.com.

## Cache

The first run of the tool can take a while since all items and recipes must be downloaded from the gw2 api.
On subsequent runs the tool will use cached versions of the item and recipe databases, stored in the 'items.bin' and 'recipes.bin' files respectively.
These files can be deleted to clear the cache.
