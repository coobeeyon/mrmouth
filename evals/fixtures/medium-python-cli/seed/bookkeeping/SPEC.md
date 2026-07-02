# Medium Python CLI Eval Spec

The code worktree contains `sales_report.py`, sample order data, and tests. The
current implementation is incomplete.

Implement `summarize_orders(rows)` and the CLI so the report handles:

- CSV columns: `order_id`, `date`, `region`, `category`, `item`, `quantity`,
  `unit_price`, `discount_pct`, `status`
- `cancelled` rows are ignored completely.
- `paid` rows add discounted revenue and units.
- `refunded` rows subtract discounted revenue and units, and increment
  `refund_count`.
- Currency values are rounded to two decimals.
- The summary includes `order_count`, `refund_count`, `net_revenue`,
  `units_sold`, `date_range`, `regions`, `categories`, and `top_category`.
- `top_category` is the category with the highest `net_revenue`; ties sort by
  category name.
- `regions` and `categories` are dictionaries keyed by name. Each entry has
  `orders`, `units`, and `net_revenue`.
- The CLI accepts a CSV path and optional `--output <path>`. Without `--output`
  it writes JSON to stdout; with `--output` it writes the same JSON to that file.

Run `./check.sh` in the code worktree and make it pass. Commit the code change
in the code worktree, then close the Litebrite item.
