import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from fulfillops.cli import build_report


DATA_DIR = Path(__file__).resolve().parents[1] / "data"


class CliTests(unittest.TestCase):
    def test_build_report_shape(self):
        report = build_report(DATA_DIR / "products.csv", DATA_DIR / "orders.csv")
        self.assertEqual(report["order_count"], 7)
        self.assertEqual(report["line_count"], 8)
        self.assertEqual(report["fully_allocated_orders"], 3)
        self.assertEqual(report["backordered_units"], 4)
        self.assertEqual(len(report["shipments"]), 6)
        self.assertEqual(report["carrier_totals"]["AirSafe"], 33.0)
        self.assertEqual(report["risk"][0]["order_id"], "O-100")

    def test_cli_writes_stdout_and_output_file(self):
        completed = subprocess.run(
            ["python3", "-m", "fulfillops.cli", str(DATA_DIR / "products.csv"), str(DATA_DIR / "orders.csv")],
            text=True,
            check=True,
            stdout=subprocess.PIPE,
        )
        stdout_report = json.loads(completed.stdout)
        with tempfile.TemporaryDirectory() as tmp:
            output_path = Path(tmp) / "report.json"
            subprocess.run(
                [
                    "python3",
                    "-m",
                    "fulfillops.cli",
                    str(DATA_DIR / "products.csv"),
                    str(DATA_DIR / "orders.csv"),
                    "--output",
                    str(output_path),
                ],
                text=True,
                check=True,
                stdout=subprocess.PIPE,
            )
            file_report = json.loads(output_path.read_text(encoding="utf-8"))
        self.assertEqual(file_report, stdout_report)


if __name__ == "__main__":
    unittest.main()
