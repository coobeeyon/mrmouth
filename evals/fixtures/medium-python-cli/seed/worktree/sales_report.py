#!/usr/bin/env python3
import argparse
import csv
import json
from pathlib import Path


def load_orders(path):
    with open(path, newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def summarize_orders(rows):
    summary = {
        "order_count": 0,
        "net_revenue": 0.0,
        "regions": {},
    }

    for row in rows:
        if row.get("status") == "cancelled":
            continue
        quantity = int(row["quantity"])
        unit_price = float(row["unit_price"])
        revenue = quantity * unit_price
        summary["order_count"] += 1
        summary["net_revenue"] += revenue
        region = row["region"]
        region_summary = summary["regions"].setdefault(
            region, {"orders": 0, "net_revenue": 0.0}
        )
        region_summary["orders"] += 1
        region_summary["net_revenue"] += revenue

    summary["net_revenue"] = round(summary["net_revenue"], 2)
    for region_summary in summary["regions"].values():
        region_summary["net_revenue"] = round(region_summary["net_revenue"], 2)
    return summary


def main(argv=None):
    parser = argparse.ArgumentParser(description="Summarize order CSV data.")
    parser.add_argument("csv_path")
    parser.add_argument("--output")
    args = parser.parse_args(argv)

    summary = summarize_orders(load_orders(args.csv_path))
    encoded = json.dumps(summary, indent=2, sort_keys=True)
    if args.output:
        Path(args.output).write_text(f"{encoded}\n", encoding="utf-8")
    else:
        print(encoded)


if __name__ == "__main__":
    main()
