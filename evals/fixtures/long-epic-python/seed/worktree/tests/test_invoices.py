import unittest

from tests.helpers import quoted
from fulfillops.invoices import build_invoices


class InvoiceTests(unittest.TestCase):
    def test_builds_one_invoice_per_order(self):
        _, lines, shipments = quoted()
        invoices = {invoice["order_id"]: invoice for invoice in build_invoices(lines, shipments)}
        self.assertEqual(set(invoices), {"O-100", "O-101", "O-102", "O-103", "O-104", "O-105", "O-106"})
        self.assertEqual(invoices["O-100"]["merchandise_subtotal"], 166.0)
        self.assertEqual(invoices["O-100"]["shipping_cost"], 33.0)
        self.assertEqual(invoices["O-100"]["total_due"], 199.0)
        self.assertEqual(invoices["O-102"]["invoice_status"], "due")
        self.assertEqual(invoices["O-104"]["shipping_cost"], 0.0)
        self.assertEqual(invoices["O-104"]["total_due"], 0.0)


if __name__ == "__main__":
    unittest.main()
