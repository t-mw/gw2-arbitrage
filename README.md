# gw2-arbitrage

Finds items in Guild Wars 2 that can be sold on the trading post for a higher price than the cost of crafting the item.

## Usage

1. Install [Rust](https://www.rust-lang.org/learn/get-started).
1. Download the code from this repository and extract it to a folder.
1. If running Windows, open Command Prompt or PowerShell from the Start Menu. PowerShell is the better choice on Windows 7. If running Linux or MacOS, open your terminal application.
1. Navigate to the folder using the terminal application.
1. Run the binary with `cargo run --release` to produce a list of items that can be crafted and immediately resold
for profit on the trading post using materials purchased from the trading post. Items with high 'profit / step'
and 'profit on cost' values generally produce a higher return for the time invested.

    ![List of items](screen1.png)

1. Pass an item id as input (e.g. `cargo run --release -- 11538`) to print a shopping list for the item, which considers
the total available liquidity for each ingredient on the trading post. By default the shopping list will assume that you
want to produce as many copies of the item as can be profitably sold on the trading post.


    ![List of materials](screen2.png)

    To limit the number of items that will be crafted a count may also be passed (e.g. `cargo run --release -- 11538 --count 100` will limit the shopping list to producing 100 items).

1. Detailed crafting instructions for the item can then be found on https://www.gw2bltc.com.

**Important**: Check that you have the recipe for the item before spending any money. Some profitable opportunities listed by the tool exist because the recipes are difficult to obtain.

Note that the shopping list may contain a mix of components (e.g. Trouser Padding) and raw materials required for that component (e.g. Bolts of Silk + Thread).
This is because the tool considers your effect on the market when you buy materials from the trading post.
For example, after buying enough Bolts of Silk and Thread to craft 100 x Trouser Padding, you may push the price of the materials high enough that it becomes cheaper to simply buy the Trouser Padding already crafted.

## Options

Run `cargo run --release -- --help` to see all available options, including CSV export.

## Cache

The first run of the tool can take a while since all items and recipes must be downloaded from the Guild Wars 2 API.
On subsequent runs the tool will use cached versions of the item and recipe databases, stored in the 'items.bin' and 'recipes.bin' files respectively.
These files can be deleted to clear the cache.
