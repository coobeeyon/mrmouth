import unittest
from pathlib import Path

from supportops.loader import load_tickets
from supportops.risk import customer_risk
from supportops.routing import apply_routing
from supportops.sla import apply_sla


ROOT = Path(__file__).resolve().parents[1]
TICKETS = ROOT / "data" / "tickets.csv"


class RiskTests(unittest.TestCase):
    def test_customer_risk_scores_and_sorts(self):
        tickets = apply_routing(apply_sla(load_tickets(TICKETS)))
        risk = customer_risk(tickets)

        self.assertEqual(
            risk,
            [
                {"customer": "Acme", "breach_count": 3, "open_escalations": 2, "risk_score": 12},
                {"customer": "Contoso", "breach_count": 2, "open_escalations": 0, "risk_score": 4},
                {"customer": "Delta", "breach_count": 1, "open_escalations": 0, "risk_score": 2},
                {"customer": "Echo", "breach_count": 1, "open_escalations": 0, "risk_score": 2},
            ],
        )


if __name__ == "__main__":
    unittest.main()
