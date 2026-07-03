def classify_ticket(ticket):
    return dict(ticket)


def apply_sla(tickets):
    return [classify_ticket(ticket) for ticket in tickets]
