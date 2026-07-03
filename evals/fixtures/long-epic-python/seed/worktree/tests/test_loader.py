import csv
import tempfile
import unittest
from pathlib import Path

from fulfillops.loader import load_order_lines, load_products


DATA_DIR = Path(__file__).resolve().parents[1] / "data"


class LoaderTests(unittest.TestCase):
    def test_loads_products_with_types(self):
        products = load_products(DATA_DIR / "products.csv")
        self.assertEqual(sorted(products), ["SKU-1", "SKU-2", "SKU-3", "SKU-4", "SKU-5"])
        self.assertEqual(products["SKU-2"]["stock"], 3)
        self.assertEqual(products["SKU-2"]["unit_price"], 39.0)
        self.assertEqual(products["SKU-2"]["weight_oz"], 18)
        self.assertTrue(products["SKU-2"]["hazmat"])

    def test_loads_order_lines_in_file_order(self):
        lines = load_order_lines(DATA_DIR / "orders.csv")
        self.assertEqual(len(lines), 8)
        self.assertEqual([line["order_id"] for line in lines[:3]], ["O-100", "O-100", "O-101"])
        self.assertEqual(lines[0]["quantity"], 2)
        self.assertTrue(lines[0]["paid"])
        self.assertFalse(lines[3]["paid"])

    def test_rejects_duplicate_skus(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "products.csv"
            with path.open("w", newline="", encoding="utf-8") as f:
                writer = csv.DictWriter(
                    f,
                    fieldnames=["sku", "name", "category", "stock", "unit_price", "weight_oz", "hazmat"],
                )
                writer.writeheader()
                writer.writerow({"sku": "X", "name": "One", "category": "c", "stock": "1", "unit_price": "1", "weight_oz": "1", "hazmat": "no"})
                writer.writerow({"sku": "X", "name": "Two", "category": "c", "stock": "1", "unit_price": "1", "weight_oz": "1", "hazmat": "no"})
            with self.assertRaises(ValueError):
                load_products(path)


if __name__ == "__main__":
    unittest.main()
