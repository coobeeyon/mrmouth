import tempfile
import unittest
from pathlib import Path

from inventory.catalog import load_catalog


ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "data" / "catalog.csv"


class CatalogTests(unittest.TestCase):
    def test_load_catalog_types_and_sorts_rows(self):
        items = load_catalog(CATALOG)

        self.assertEqual([item["sku"] for item in items], ["A100", "A200", "B100", "C100"])
        self.assertEqual(items[0]["name"], "Black Tea")
        self.assertEqual(items[0]["category"], "Beverages")
        self.assertEqual(items[0]["on_hand"], 8)
        self.assertEqual(items[0]["reorder_point"], 10)
        self.assertEqual(items[0]["reorder_qty"], 24)
        self.assertEqual(items[0]["unit_cost"], 2.5)

    def test_load_catalog_rejects_duplicate_skus(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "catalog.csv"
            path.write_text(
                "\n".join(
                    [
                        "sku,name,category,on_hand,reorder_point,reorder_qty,unit_cost",
                        "A100,Black Tea,Beverages,8,10,24,2.50",
                        "A100,Tea Duplicate,Beverages,1,2,3,4.00",
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            with self.assertRaises(ValueError):
                load_catalog(path)

    def test_load_catalog_rejects_missing_columns(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "catalog.csv"
            path.write_text("sku,name\nA100,Black Tea\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                load_catalog(path)


if __name__ == "__main__":
    unittest.main()
