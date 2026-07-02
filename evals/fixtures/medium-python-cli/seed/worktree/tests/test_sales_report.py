import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import sales_report


ROOT = Path(__file__).resolve().parents[1]
ORDERS = ROOT / "data" / "orders.csv"


EXPECTED = {
    "order_count": 6,
    "refund_count": 1,
    "net_revenue": 109.0,
    "units_sold": 15,
    "date_range": {"start": "2026-01-05", "end": "2026-01-10"},
    "regions": {
        "North": {"orders": 3, "units": 7, "net_revenue": 83.0},
        "South": {"orders": 1, "units": 5, "net_revenue": 20.0},
        "West": {"orders": 2, "units": 3, "net_revenue": 6.0},
    },
    "categories": {
        "Books": {"orders": 3, "units": 4, "net_revenue": 45.0},
        "Games": {"orders": 1, "units": 5, "net_revenue": 20.0},
        "Kitchen": {"orders": 2, "units": 6, "net_revenue": 44.0},
    },
    "top_category": "Books",
}


class SalesReportTests(unittest.TestCase):
    def test_summarize_orders_matches_expected_contract(self):
        rows = sales_report.load_orders(ORDERS)
        self.assertEqual(sales_report.summarize_orders(rows), EXPECTED)

    def test_stdout_cli_emits_expected_json(self):
        completed = subprocess.run(
            [sys.executable, "sales_report.py", "data/orders.csv"],
            cwd=ROOT,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        )
        self.assertEqual(json.loads(completed.stdout), EXPECTED)

    def test_output_file_cli_emits_expected_json(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "summary.json"
            subprocess.run(
                [
                    sys.executable,
                    "sales_report.py",
                    "data/orders.csv",
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                check=True,
            )
            self.assertEqual(json.loads(output.read_text(encoding="utf-8")), EXPECTED)


if __name__ == "__main__":
    unittest.main()
