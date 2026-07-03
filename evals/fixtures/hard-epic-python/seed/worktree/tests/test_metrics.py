import unittest
from pathlib import Path

from supportops.loader import load_tickets
from supportops.metrics import queue_metrics
from supportops.routing import apply_routing
from supportops.sla import apply_sla


ROOT = Path(__file__).resolve().parents[1]
TICKETS = ROOT / "data" / "tickets.csv"


class MetricsTests(unittest.TestCase):
    def test_queue_metrics_aggregate_breaches_and_averages(self):
        tickets = apply_routing(apply_sla(load_tickets(TICKETS)))
        metrics = queue_metrics(tickets)

        self.assertEqual(metrics["Security"], {
            "ticket_count": 2,
            "open_count": 1,
            "response_breaches": 0,
            "resolution_breaches": 1,
            "average_resolution_minutes": 1600.0,
        })
        self.assertEqual(metrics["Billing"], {
            "ticket_count": 3,
            "open_count": 1,
            "response_breaches": 3,
            "resolution_breaches": 2,
            "average_resolution_minutes": 2300.0,
        })
        self.assertEqual(metrics["EU Support"]["average_resolution_minutes"], 2500.0)
        self.assertEqual(metrics["NA Support"]["average_resolution_minutes"], 470.0)


if __name__ == "__main__":
    unittest.main()
