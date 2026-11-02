# Golden fixture: list[float] (I#569) — literal, indexing, len, index-set,
# append, iteration, and summing. A `double` element shares the uniform list
# word and is carried by the `_f64` runtime entry points. Values are chosen to
# format exactly (no platform-sensitive rounding).
def main():
    list[float] xs = [1.0, 2.0, 3.0]
    print(len(xs))  # 3
    print(xs[0])  # 1.0
    print(xs[2])  # 3.0
    xs[1] = 9.5
    print(xs[1])  # 9.5
    xs.append(4.0)
    print(len(xs))  # 4
    float total = 0.0
    for x in xs:
        total = total + x
    print(total)  # 17.5
