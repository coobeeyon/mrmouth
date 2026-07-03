# Inventory Planning Eval Spec

The code worktree contains a small Python package named `inventory`, CSV input
data, and focused tests. The current implementation is incomplete.

## 1. Catalog Loading

`inventory.catalog.load_catalog(path)` reads `catalog.csv` with columns:

- `sku`
- `name`
- `category`
- `on_hand`
- `reorder_point`
- `reorder_qty`
- `unit_cost`

Return a list of dictionaries sorted by `sku`. Numeric fields are typed as
integers except `unit_cost`, which is a float rounded to two decimals. Raise
`ValueError` for missing required columns or duplicate SKUs.

## 2. Stock Movements

`inventory.stock.load_movements(path)` reads `movements.csv` with columns
`sku`, `movement_type`, and `quantity`. Quantity is an integer.

`inventory.stock.apply_movements(catalog_items, movements)` returns a list of
inventory dictionaries sorted by `sku`. Start from the catalog `on_hand` values.
Movement types behave as follows:

- `receipt` adds to `on_hand` and `units_received`.
- `sale` subtracts from `on_hand` and adds to `units_sold`.
- `adjustment` adds the signed quantity to `on_hand` and to `adjustments`.

Reject unknown SKUs or movement types with `ValueError`.

## 3. Reorder Planning

`inventory.reorder.build_reorder_plan(inventory_items)` returns:

```json
{
  "items": [
    {
      "sku": "C100",
      "name": "USB-C Cable",
      "category": "Electronics",
      "on_hand": 2,
      "reorder_point": 6,
      "reorder_qty": 10,
      "estimated_cost": 47.5
    }
  ],
  "total_cost": 72.5
}
```

Include items whose final `on_hand` is less than or equal to `reorder_point`.
Sort items by `category`, then `sku`. Round money to two decimals.

## 4. CLI Reporting

`python3 -m inventory.cli <catalog.csv> <movements.csv> [--output report.json]`
loads the catalog, applies movements, builds the reorder plan, and emits JSON.
Without `--output`, write JSON to stdout. With `--output`, write the same JSON
to that file.

The report shape is:

```json
{
  "item_count": 4,
  "total_on_hand": 34,
  "movement_summary": {
    "units_sold": 6,
    "units_received": 10,
    "adjustments": -8
  },
  "categories": {
    "Beverages": {"sku_count": 2, "on_hand": 30}
  },
  "reorder": {"items": [], "total_cost": 0.0}
}
```

Run `./check.sh` in the code worktree when all children are complete.
