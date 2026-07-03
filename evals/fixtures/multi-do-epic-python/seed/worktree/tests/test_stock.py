import tempfile
import unittest
from pathlib import Path

from inventory.catalog import load_catalog
from inventory.stock import apply_movements, load_movements


ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "data" / "catalog.csv"
MOVEMENTS = ROOT / "data" / "movements.csv"


class StockTests(unittest.TestCase):
    def test_load_movements_types_quantities(self):
        movements = load_movements(MOVEMENTS)

        self.assertEqual(movements[0], {"sku": "A100", "movement_type": "sale", "quantity": 3})
        self.assertEqual(movements[3], {"sku": "C100", "movement_type": "adjustment", "quantity": -8})

    def test_apply_movements_updates_inventory_and_summaries(self):
        inventory = apply_movements(load_catalog(CATALOG), load_movements(MOVEMENTS))
        by_sku = {item["sku"]: item for item in inventory}

        self.assertEqual([item["sku"] for item in inventory], ["A100", "A200", "B100", "C100"])
        self.assertEqual(by_sku["A100"]["on_hand"], 11)
        self.assertEqual(by_sku["A100"]["units_sold"], 3)
        self.assertEqual(by_sku["A100"]["units_received"], 6)
        self.assertEqual(by_sku["A100"]["adjustments"], 0)
        self.assertEqual(by_sku["A200"]["on_hand"], 19)
        self.assertEqual(by_sku["B100"]["on_hand"], 2)
        self.assertEqual(by_sku["C100"]["on_hand"], 2)
        self.assertEqual(by_sku["C100"]["units_sold"], 2)
        self.assertEqual(by_sku["C100"]["adjustments"], -8)

    def test_apply_movements_rejects_unknown_sku(self):
        movements = [{"sku": "NOPE", "movement_type": "sale", "quantity": 1}]
        with self.assertRaises(ValueError):
            apply_movements(load_catalog(CATALOG), movements)

    def test_apply_movements_rejects_unknown_type(self):
        movements = [{"sku": "A100", "movement_type": "transfer", "quantity": 1}]
        with self.assertRaises(ValueError):
            apply_movements(load_catalog(CATALOG), movements)

    def test_load_movements_rejects_missing_columns(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "movements.csv"
            path.write_text("sku,quantity\nA100,1\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                load_movements(path)


if __name__ == "__main__":
    unittest.main()
