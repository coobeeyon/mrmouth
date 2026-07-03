import unittest
from pathlib import Path

from inventory.catalog import load_catalog
from inventory.reorder import build_reorder_plan
from inventory.stock import apply_movements, load_movements


ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "data" / "catalog.csv"
MOVEMENTS = ROOT / "data" / "movements.csv"


class ReorderTests(unittest.TestCase):
    def test_build_reorder_plan_uses_final_stock_and_sorts_by_category_then_sku(self):
        inventory = apply_movements(load_catalog(CATALOG), load_movements(MOVEMENTS))
        plan = build_reorder_plan(inventory)

        self.assertEqual(
            plan,
            {
                "items": [
                    {
                        "sku": "C100",
                        "name": "USB-C Cable",
                        "category": "Electronics",
                        "on_hand": 2,
                        "reorder_point": 6,
                        "reorder_qty": 10,
                        "estimated_cost": 47.5,
                    },
                    {
                        "sku": "B100",
                        "name": "Notebook",
                        "category": "Stationery",
                        "on_hand": 2,
                        "reorder_point": 5,
                        "reorder_qty": 20,
                        "estimated_cost": 25.0,
                    },
                ],
                "total_cost": 72.5,
            },
        )


if __name__ == "__main__":
    unittest.main()
