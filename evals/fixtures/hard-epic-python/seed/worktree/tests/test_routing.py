import unittest
from pathlib import Path

from supportops.loader import load_tickets
from supportops.routing import apply_routing, route_ticket
from supportops.sla import apply_sla


ROOT = Path(__file__).resolve().parents[1]
TICKETS = ROOT / "data" / "tickets.csv"


class RoutingTests(unittest.TestCase):
    def test_route_ticket_maps_categories(self):
        self.assertEqual(route_ticket({"category": "security", "region": "EU"}), "Security")
        self.assertEqual(route_ticket({"category": "billing", "region": "NA"}), "Billing")
        self.assertEqual(route_ticket({"category": "technical", "region": "APAC"}), "APAC Support")

    def test_apply_routing_adds_queue_to_every_ticket(self):
        tickets = apply_routing(apply_sla(load_tickets(TICKETS)))
        queues = {ticket["ticket_id"]: ticket["queue"] for ticket in tickets}

        self.assertEqual(queues["T100"], "Security")
        self.assertEqual(queues["T101"], "Billing")
        self.assertEqual(queues["T102"], "EU Support")
        self.assertEqual(queues["T106"], "NA Support")


if __name__ == "__main__":
    unittest.main()
