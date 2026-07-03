import unittest
from pathlib import Path

from supportops.loader import load_tickets
from supportops.sla import apply_sla, classify_ticket


ROOT = Path(__file__).resolve().parents[1]
TICKETS = ROOT / "data" / "tickets.csv"


class SlaTests(unittest.TestCase):
    def test_classify_ticket_sets_targets_and_breaches(self):
        tickets = {ticket["ticket_id"]: ticket for ticket in apply_sla(load_tickets(TICKETS))}

        self.assertEqual(tickets["T100"]["response_target_minutes"], 60)
        self.assertEqual(tickets["T100"]["resolution_target_minutes"], 480)
        self.assertFalse(tickets["T100"]["response_breached"])
        self.assertFalse(tickets["T100"]["resolution_breached"])
        self.assertTrue(tickets["T100"]["open_escalation"])

        self.assertTrue(tickets["T101"]["response_breached"])
        self.assertTrue(tickets["T101"]["resolution_breached"])
        self.assertTrue(tickets["T105"]["response_breached"])
        self.assertTrue(tickets["T105"]["open_escalation"])
        self.assertFalse(tickets["T106"]["response_breached"])
        self.assertFalse(tickets["T106"]["resolution_breached"])

    def test_classify_ticket_rejects_unknown_priority(self):
        with self.assertRaises(ValueError):
            classify_ticket({"priority": "none", "status": "open", "plan": "free"})


if __name__ == "__main__":
    unittest.main()
