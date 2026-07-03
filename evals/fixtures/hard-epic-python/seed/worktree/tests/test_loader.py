import tempfile
import unittest
from pathlib import Path

from supportops.loader import load_tickets


ROOT = Path(__file__).resolve().parents[1]
TICKETS = ROOT / "data" / "tickets.csv"


class LoaderTests(unittest.TestCase):
    def test_load_tickets_types_and_sorts(self):
        tickets = load_tickets(TICKETS)

        self.assertEqual([ticket["ticket_id"] for ticket in tickets], [f"T10{i}" for i in range(8)])
        self.assertEqual(tickets[0]["response_minutes"], 45)
        self.assertIsNone(tickets[0]["resolution_minutes"])
        self.assertEqual(tickets[1]["resolution_minutes"], 1500)

    def test_load_tickets_rejects_duplicate_ids(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "tickets.csv"
            path.write_text(
                "\n".join(
                    [
                        "ticket_id,opened_at,customer,plan,region,category,priority,status,response_minutes,resolution_minutes",
                        "T100,2026-02-01T09:00:00,Acme,enterprise,NA,security,urgent,open,45,",
                        "T100,2026-02-01T10:00:00,Acme,enterprise,NA,billing,high,closed,300,1500",
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            with self.assertRaises(ValueError):
                load_tickets(path)

    def test_load_tickets_rejects_missing_columns(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "tickets.csv"
            path.write_text("ticket_id,customer\nT100,Acme\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                load_tickets(path)


if __name__ == "__main__":
    unittest.main()
