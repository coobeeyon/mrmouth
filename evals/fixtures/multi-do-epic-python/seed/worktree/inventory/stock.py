import csv


def load_movements(path):
    with open(path, newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def apply_movements(catalog_items, movements):
    return list(catalog_items)
