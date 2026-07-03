# Fulfillment Operations Eval Spec

The code worktree contains a Python package named `fulfillops`, sample product
and order data, and focused tests. The current implementation is incomplete.

## 1. Loading

`fulfillops.loader.load_products(path)` reads `products.csv` with columns:
`sku`, `name`, `category`, `stock`, `unit_price`, `weight_oz`, and `hazmat`.

Return a dictionary keyed by `sku`. Convert `stock` and `weight_oz` to integers,
`unit_price` to a float rounded to two decimals, and `hazmat` to a boolean.
Reject duplicate SKUs and missing required columns with `ValueError`.

`fulfillops.loader.load_order_lines(path)` reads `orders.csv` with columns:
`order_id`, `customer`, `region`, `service_level`, `sku`, `quantity`, and
`paid`. Return rows in file order. Convert `quantity` to an integer and `paid`
to a boolean. Reject missing required columns.

## 2. Catalog Enrichment

`fulfillops.catalog.enrich_lines(lines, products)` returns copies of order lines
with product fields: `name`, `category`, `unit_price`, `weight_oz`, `hazmat`,
and `requested_subtotal`. Reject unknown SKUs with `ValueError`.

## 3. Stock Allocation

`fulfillops.allocation.allocate_lines(lines, products)` allocates stock in file
order. Return line copies with `allocated_quantity`, `backordered_quantity`, and
`fully_allocated`. Do not mutate the input product stock values.

## 4. Shipments

`fulfillops.shipments.build_shipments(lines)` groups allocated quantities by
`order_id`. Exclude orders with zero allocated units. Each shipment has
`order_id`, `customer`, `region`, `service_level`, `item_count`,
`total_weight_oz`, `contains_hazmat`, and `status`. Status is `ready` when every
line for that order is fully allocated, otherwise `partial`.

## 5. Carrier Quotes

`fulfillops.carriers.quote_shipments(shipments)` returns shipment copies with
`carrier` and `shipping_cost`. Express hazmat shipments use `AirSafe` at
`18 + 0.10 * total_weight_oz`; other express shipments use `AirFast` at
`12 + 0.08 * total_weight_oz`; standard hazmat shipments use `GroundHaz` at
`10 + 0.06 * total_weight_oz`; other standard shipments use `Ground` at
`6 + 0.04 * total_weight_oz`. Round costs to two decimals.

## 6. Invoices

`fulfillops.invoices.build_invoices(lines, quoted_shipments)` returns one
invoice per order. Use allocated quantities for merchandise subtotal, add the
quoted shipment cost when present, and set `invoice_status` to `paid` or `due`
from the order's `paid` flag. Include orders even when they have no shipment.

## 7. Backorders

`fulfillops.backorders.backorder_plan(lines)` returns one entry per SKU with
backordered units. Each entry has `sku`, `total_backordered`, and
`affected_orders`. Sort by descending backordered units, then SKU.

## 8. Risk Scoring

`fulfillops.risk.order_risk(lines, invoices)` returns orders with
`risk_score >= 4`. Score five points for express service, four for any
backordered units, three for unpaid orders, one for hazmat, and one for
merchandise subtotal at least 100. Sort by descending score, then `order_id`.

## 9. Metrics

`fulfillops.metrics.summary(lines, quoted_shipments, invoices, backorders)`
returns a dictionary with `order_count`, `line_count`, `fully_allocated_orders`,
`backordered_units`, and `carrier_totals`. Carrier totals sum shipping costs by
carrier and are rounded to two decimals.

## 10. CLI Reporting

`python3 -m fulfillops.cli <products.csv> <orders.csv> [--output report.json]`
emits JSON with:

```json
{
  "order_count": 7,
  "line_count": 8,
  "fully_allocated_orders": 3,
  "backordered_units": 4,
  "carrier_totals": {},
  "shipments": [],
  "invoices": [],
  "backorders": [],
  "risk": []
}
```

Without `--output`, write JSON to stdout. With `--output`, write the same JSON
to that file. Run `./check.sh` in the code worktree when all children are
complete.
