import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from inventory.cli import build_report


ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "data" / "catalog.csv"
MOVEMENTS = ROOT / "data" / "movements.csv"


EXPECTED = {
    "item_count": 4,
    "total_on_hand": 34,
    "movement_summary": {
        "units_sold": 6,
        "units_received": 10,
        "adjustments": -8,
    },
    "categories": {
        "Beverages": {"sku_count": 2, "on_hand": 30},
        "Electronics": {"sku_count": 1, "on_hand": 2},
        "Stationery": {"sku_count": 1, "on_hand": 2},
    },
    "reorder": {
        "items": [
            {
                "sku": "C100",
                "name": "USB-C Cable",
                "category": "Electronics",
                "on_hand": 2,
                "reorder_point": 6,
                "reorder_qty": 10,
                "estimated_cost": 47.5,
            },
            {
                "sku": "B100",
                "name": "Notebook",
                "category": "Stationery",
                "on_hand": 2,
                "reorder_point": 5,
                "reorder_qty": 20,
                "estimated_cost": 25.0,
            },
        ],
        "total_cost": 72.5,
    },
}


class CliTests(unittest.TestCase):
    def test_build_report_matches_expected_contract(self):
        self.assertEqual(build_report(CATALOG, MOVEMENTS), EXPECTED)

    def test_stdout_cli_emits_expected_json(self):
        completed = subprocess.run(
            [sys.executable, "-m", "inventory.cli", "data/catalog.csv", "data/movements.csv"],
            cwd=ROOT,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        )
        self.assertEqual(json.loads(completed.stdout), EXPECTED)

    def test_output_file_cli_emits_expected_json(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "report.json"
            subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "inventory.cli",
                    "data/catalog.csv",
                    "data/movements.csv",
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                check=True,
            )
            self.assertEqual(json.loads(output.read_text(encoding="utf-8")), EXPECTED)


if __name__ == "__main__":
    unittest.main()
