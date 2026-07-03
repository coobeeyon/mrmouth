import unittest

from tests.helpers import allocated
from fulfillops.shipments import build_shipments


class ShipmentTests(unittest.TestCase):
    def test_groups_allocated_units_by_order(self):
        _, lines = allocated()
        shipments = {shipment["order_id"]: shipment for shipment in build_shipments(lines)}
        self.assertNotIn("O-104", shipments)
        self.assertEqual(shipments["O-100"]["item_count"], 5)
        self.assertEqual(shipments["O-100"]["total_weight_oz"], 150)
        self.assertTrue(shipments["O-100"]["contains_hazmat"])
        self.assertEqual(shipments["O-100"]["status"], "partial")
        self.assertEqual(shipments["O-101"]["status"], "ready")
        self.assertEqual(shipments["O-106"]["status"], "partial")


if __name__ == "__main__":
    unittest.main()
