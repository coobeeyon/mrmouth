import unittest

from tests.helpers import enriched
from fulfillops.allocation import allocate_lines


class AllocationTests(unittest.TestCase):
    def test_allocates_in_file_order_without_mutating_stock(self):
        products, lines = enriched()
        allocated = allocate_lines(lines, products)
        self.assertEqual(products["SKU-2"]["stock"], 3)
        by_order_sku = {
            (line["order_id"], line["sku"]): line
            for line in allocated
        }
        self.assertEqual(by_order_sku[("O-100", "SKU-2")]["allocated_quantity"], 3)
        self.assertEqual(by_order_sku[("O-100", "SKU-2")]["backordered_quantity"], 1)
        self.assertFalse(by_order_sku[("O-100", "SKU-2")]["fully_allocated"])
        self.assertEqual(by_order_sku[("O-104", "SKU-2")]["allocated_quantity"], 0)
        self.assertEqual(by_order_sku[("O-106", "SKU-4")]["backordered_quantity"], 1)


if __name__ == "__main__":
    unittest.main()
