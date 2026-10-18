# Golden fixture: list[int64] — literal, indexing, len, set, append, iteration.
def main():
    auto xs = [10, 20, 30]
    print(len(xs))
    print(xs[0])
    print(xs[2])
    xs[1] = 99
    print(xs[1])
    xs.append(40)
    print(len(xs))
    auto total = 0
    for x in xs:
        total = total + x
    print(total)
