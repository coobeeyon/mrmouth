import csv


REQUIRED_COLUMNS = {
    "sku",
    "name",
    "category",
    "on_hand",
    "reorder_point",
    "reorder_qty",
    "unit_cost",
}


def load_catalog(path):
    with open(path, newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        return list(reader)
