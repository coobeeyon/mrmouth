import unittest

from tests.helpers import loaded
from fulfillops.catalog import enrich_lines


class CatalogTests(unittest.TestCase):
    def test_enriches_lines_with_product_fields(self):
        products, lines = loaded()
        enriched = enrich_lines(lines, products)
        self.assertEqual(enriched[0]["name"], "Desk Lamp")
        self.assertEqual(enriched[0]["category"], "home")
        self.assertEqual(enriched[0]["unit_price"], 24.5)
        self.assertEqual(enriched[0]["weight_oz"], 48)
        self.assertFalse(enriched[0]["hazmat"])
        self.assertEqual(enriched[1]["requested_subtotal"], 156.0)

    def test_rejects_unknown_sku(self):
        products, lines = loaded()
        broken = [dict(lines[0], sku="missing")]
        with self.assertRaises(ValueError):
            enrich_lines(broken, products)


if __name__ == "__main__":
    unittest.main()
