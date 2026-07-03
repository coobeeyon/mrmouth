import unittest

from tests.helpers import invoiced
from fulfillops.risk import order_risk


class RiskTests(unittest.TestCase):
    def test_scores_risky_orders(self):
        _, lines, _, invoices = invoiced()
        risks = order_risk(lines, invoices)
        self.assertEqual(
            [(entry["order_id"], entry["risk_score"]) for entry in risks],
            [("O-100", 11), ("O-104", 9), ("O-106", 9), ("O-102", 8), ("O-103", 5)],
        )
        self.assertEqual(risks[0]["reasons"], ["express", "backordered", "hazmat", "high_value"])


if __name__ == "__main__":
    unittest.main()
