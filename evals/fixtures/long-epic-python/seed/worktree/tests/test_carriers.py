import unittest

from tests.helpers import allocated
from fulfillops.carriers import quote_shipments
from fulfillops.shipments import build_shipments


class CarrierTests(unittest.TestCase):
    def test_quotes_by_service_and_hazmat(self):
        _, lines = allocated()
        quoted = {shipment["order_id"]: shipment for shipment in quote_shipments(build_shipments(lines))}
        self.assertEqual(quoted["O-100"]["carrier"], "AirSafe")
        self.assertEqual(quoted["O-100"]["shipping_cost"], 33.0)
        self.assertEqual(quoted["O-102"]["carrier"], "AirFast")
        self.assertEqual(quoted["O-102"]["shipping_cost"], 13.76)
        self.assertEqual(quoted["O-103"]["carrier"], "GroundHaz")
        self.assertEqual(quoted["O-103"]["shipping_cost"], 14.8)
        self.assertEqual(quoted["O-105"]["carrier"], "Ground")
        self.assertEqual(quoted["O-105"]["shipping_cost"], 7.28)


if __name__ == "__main__":
    unittest.main()
