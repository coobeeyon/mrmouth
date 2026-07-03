# Support Operations Eval Spec

The code worktree contains a Python package named `supportops`, sample ticket
data, and focused tests. The current implementation is incomplete.

## 1. Ticket Loading

`supportops.loader.load_tickets(path)` reads `tickets.csv` with columns:
`ticket_id`, `opened_at`, `customer`, `plan`, `region`, `category`, `priority`,
`status`, `response_minutes`, and `resolution_minutes`.

Return dictionaries sorted by `ticket_id`. Convert minute fields to integers;
blank `resolution_minutes` becomes `None`. Reject missing required columns and
duplicate ticket ids with `ValueError`.

## 2. SLA Classification

`supportops.sla.classify_ticket(ticket)` returns a copy with:

- `response_target_minutes`
- `resolution_target_minutes`
- `response_breached`
- `resolution_breached`
- `open_escalation`

Response targets are urgent 60, high 240, normal 480. Resolution targets are
urgent 480, high 1440, normal 2880. Resolution breach is false when resolution
minutes is `None`. `open_escalation` is true only for open enterprise tickets
with urgent or high priority.

`supportops.sla.apply_sla(tickets)` classifies each ticket and preserves sort
order.

## 3. Queue Routing

`supportops.routing.route_ticket(ticket)` returns:

- `Security` for security tickets
- `Billing` for billing tickets
- `<region> Support` for all other categories

`supportops.routing.apply_routing(tickets)` returns copies with `queue`.

## 4. Queue Metrics

`supportops.metrics.queue_metrics(tickets)` returns a dictionary keyed by queue.
Each queue entry has `ticket_count`, `open_count`, `response_breaches`,
`resolution_breaches`, and `average_resolution_minutes`. Averages use closed
tickets with a resolution value and are rounded to one decimal.

## 5. Customer Risk

`supportops.risk.customer_risk(tickets)` returns a list of customers with
`risk_score >= 2`. For each customer, compute:

- `breach_count`: response breaches plus resolution breaches
- `open_escalations`
- `risk_score`: `breach_count * 2 + open_escalations * 3`

Sort by descending `risk_score`, then customer name.

## 6. CLI Reporting

`python3 -m supportops.cli <tickets.csv> [--output report.json]` emits JSON:

```json
{
  "ticket_count": 8,
  "open_count": 3,
  "breach_totals": {"response": 4, "resolution": 3},
  "queues": {},
  "customer_risk": []
}
```

Without `--output`, write JSON to stdout. With `--output`, write the same JSON
to that file. Run `./check.sh` in the code worktree when all children are
complete.
