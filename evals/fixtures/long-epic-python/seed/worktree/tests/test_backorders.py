import unittest

from tests.helpers import allocated
from fulfillops.backorders import backorder_plan


class BackorderTests(unittest.TestCase):
    def test_groups_backorders_by_sku(self):
        _, lines = allocated()
        plan = backorder_plan(lines)
        self.assertEqual(
            plan,
            [
                {"sku": "SKU-2", "total_backordered": 2, "affected_orders": ["O-100", "O-104"]},
                {"sku": "SKU-4", "total_backordered": 1, "affected_orders": ["O-106"]},
                {"sku": "SKU-5", "total_backordered": 1, "affected_orders": ["O-103"]},
            ],
        )


if __name__ == "__main__":
    unittest.main()
