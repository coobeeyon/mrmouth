import argparse
import json
from pathlib import Path

from .catalog import load_catalog
from .stock import apply_movements, load_movements
from .reorder import build_reorder_plan


def build_report(catalog_path, movements_path):
    catalog = load_catalog(catalog_path)
    inventory = apply_movements(catalog, load_movements(movements_path))
    return {
        "item_count": len(inventory),
        "total_on_hand": sum(int(item["on_hand"]) for item in inventory),
        "movement_summary": {
            "units_sold": 0,
            "units_received": 0,
            "adjustments": 0,
        },
        "categories": {},
        "reorder": build_reorder_plan(inventory),
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description="Build an inventory planning report.")
    parser.add_argument("catalog_csv")
    parser.add_argument("movements_csv")
    parser.add_argument("--output")
    args = parser.parse_args(argv)

    report = build_report(args.catalog_csv, args.movements_csv)
    encoded = json.dumps(report, indent=2, sort_keys=True)
    if args.output:
        Path(args.output).write_text(f"{encoded}\n", encoding="utf-8")
    else:
        print(encoded)


if __name__ == "__main__":
    main()
