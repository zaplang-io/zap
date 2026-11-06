# Golden fixture: a hand-written `__str__`/`__repr__` on a tier-3 data-carrying
# enum still overrides the compiler's default structural renderer (#706,
# D-0124). The dunder is resolved BEFORE the synthesized tag-dispatch renderer,
# exactly as for a struct: `__str__` is preferred over `__repr__`, and the
# override applies everywhere the protocol is used (print/str/f-string).
enum Money:
    Cash(int64 cents)
    Card(str last4)

    def __str__(self) -> str:
        return "MONEY"


enum Ticket:
    Open(int64 id)
    Closed

    def __repr__(self) -> str:
        return "TICKET"


def main():
    print(Money.Cash(cents=100))  # MONEY  (__str__ overrides the default render)
    print(str(Money.Card(last4="4242")))  # MONEY
    print(f"{Money.Cash(cents=5)}")  # MONEY
    print(Ticket.Open(id=7))  # TICKET  (__repr__ used when no __str__)
    print(Ticket.Closed)  # TICKET
