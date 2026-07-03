import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from supportops.cli import build_report


ROOT = Path(__file__).resolve().parents[1]
TICKETS = ROOT / "data" / "tickets.csv"


class CliTests(unittest.TestCase):
    def test_build_report_matches_contract(self):
        report = build_report(TICKETS)

        self.assertEqual(report["ticket_count"], 8)
        self.assertEqual(report["open_count"], 3)
        self.assertEqual(report["breach_totals"], {"response": 4, "resolution": 3})
        self.assertEqual(report["queues"]["Billing"]["ticket_count"], 3)
        self.assertEqual(report["queues"]["Security"]["open_count"], 1)
        self.assertEqual(report["customer_risk"][0]["customer"], "Acme")
        self.assertEqual(report["customer_risk"][0]["risk_score"], 12)

    def test_stdout_cli_emits_json(self):
        completed = subprocess.run(
            [sys.executable, "-m", "supportops.cli", "data/tickets.csv"],
            cwd=ROOT,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        )
        self.assertEqual(json.loads(completed.stdout), build_report(TICKETS))

    def test_output_file_cli_emits_same_json(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "report.json"
            subprocess.run(
                [sys.executable, "-m", "supportops.cli", "data/tickets.csv", "--output", str(output)],
                cwd=ROOT,
                check=True,
            )
            self.assertEqual(json.loads(output.read_text(encoding="utf-8")), build_report(TICKETS))


if __name__ == "__main__":
    unittest.main()
