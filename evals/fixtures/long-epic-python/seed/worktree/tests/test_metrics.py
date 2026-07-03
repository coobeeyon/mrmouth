import unittest

from tests.helpers import backorders
from fulfillops.metrics import summary


class MetricsTests(unittest.TestCase):
    def test_summarizes_fulfillment(self):
        _, lines, shipments, invoices, plan = backorders()
        result = summary(lines, shipments, invoices, plan)
        self.assertEqual(result["order_count"], 7)
        self.assertEqual(result["line_count"], 8)
        self.assertEqual(result["fully_allocated_orders"], 3)
        self.assertEqual(result["backordered_units"], 4)
        self.assertEqual(
            result["carrier_totals"],
            {"AirFast": 27.52, "AirSafe": 33.0, "Ground": 16.48, "GroundHaz": 14.8},
        )


if __name__ == "__main__":
    unittest.main()
