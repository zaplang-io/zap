# diff: match
# List and dict comprehensions with a filter, over the shared subset (D-0026).
# Zap comprehensions take a single `for` plus an optional `if` (GRAMMAR.ebnf
# comp_for). Results are reduced to scalars via len() and indexing so the check
# never depends on container repr formatting. CPython is the oracle.
def main():
    auto squares = [x * x for x in range(6)]
    print(len(squares))
    print(squares[0])
    print(squares[5])
    auto evens = [x for x in range(10) if x % 2 == 0]
    print(len(evens))
    print(evens[0])
    print(evens[4])
    auto lookup = {k: k * k for k in range(5)}
    print(len(lookup))
    print(lookup[0])
    print(lookup[4])
