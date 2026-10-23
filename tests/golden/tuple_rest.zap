# Golden fixture: a single `*rest` in a destructuring target captures the
# remaining elements as a sub-tuple (D-0049).
def main():
    auto first, *rest = (10, 20, 30, 40)
    print(first)
    print(rest[0])
    print(rest[1])
    print(rest[2])
