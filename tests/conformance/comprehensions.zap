# claim: COMP-1
# SPEC §8 (D-0026): a list comprehension `[expr for x in it]` builds a list by
# evaluating `expr` over the iterable, and an `if` clause filters elements.
# Observations are reduced to order-independent scalars (a length and a sum).
def main():
    auto squares = [x * x for x in range(5)]
    auto total = 0
    for s in squares:
        total = total + s
    print(total)
    auto evens = [x for x in range(10) if x % 2 == 0]
    print(len(evens))
    auto esum = 0
    for e in evens:
        esum = esum + e
    print(esum)
