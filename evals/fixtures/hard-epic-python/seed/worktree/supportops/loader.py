import csv


REQUIRED_COLUMNS = {
    "ticket_id",
    "opened_at",
    "customer",
    "plan",
    "region",
    "category",
    "priority",
    "status",
    "response_minutes",
    "resolution_minutes",
}


def load_tickets(path):
    with open(path, newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))
