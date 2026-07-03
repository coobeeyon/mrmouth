from pathlib import Path

from fulfillops.allocation import allocate_lines
from fulfillops.backorders import backorder_plan
from fulfillops.carriers import quote_shipments
from fulfillops.catalog import enrich_lines
from fulfillops.invoices import build_invoices
from fulfillops.loader import load_order_lines, load_products
from fulfillops.shipments import build_shipments


DATA_DIR = Path(__file__).resolve().parents[1] / "data"


def loaded():
    products = load_products(DATA_DIR / "products.csv")
    lines = load_order_lines(DATA_DIR / "orders.csv")
    return products, lines


def enriched():
    products, lines = loaded()
    return products, enrich_lines(lines, products)


def allocated():
    products, lines = enriched()
    return products, allocate_lines(lines, products)


def quoted():
    products, lines = allocated()
    shipments = quote_shipments(build_shipments(lines))
    return products, lines, shipments


def invoiced():
    products, lines, shipments = quoted()
    invoices = build_invoices(lines, shipments)
    return products, lines, shipments, invoices


def backorders():
    products, lines, shipments, invoices = invoiced()
    return products, lines, shipments, invoices, backorder_plan(lines)
