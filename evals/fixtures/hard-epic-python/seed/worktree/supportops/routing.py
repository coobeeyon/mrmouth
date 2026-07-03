def route_ticket(ticket):
    return ticket.get("category", "")


def apply_routing(tickets):
    return [{**ticket, "queue": route_ticket(ticket)} for ticket in tickets]
