import argparse
import json
from pathlib import Path

from .loader import load_tickets
from .metrics import queue_metrics
from .risk import customer_risk
from .routing import apply_routing
from .sla import apply_sla


def build_report(path):
    tickets = apply_routing(apply_sla(load_tickets(path)))
    return {
        "ticket_count": len(tickets),
        "open_count": 0,
        "breach_totals": {"response": 0, "resolution": 0},
        "queues": queue_metrics(tickets),
        "customer_risk": customer_risk(tickets),
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description="Build a support operations report.")
    parser.add_argument("tickets_csv")
    parser.add_argument("--output")
    args = parser.parse_args(argv)

    report = build_report(args.tickets_csv)
    encoded = json.dumps(report, indent=2, sort_keys=True)
    if args.output:
        Path(args.output).write_text(f"{encoded}\n", encoding="utf-8")
    else:
        print(encoded)


if __name__ == "__main__":
    main()
