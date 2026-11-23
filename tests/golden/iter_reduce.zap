# Golden fixture: iterable min/max (+ key=/default=), sum(start), enumerate, zip
# (#1299, #1302).

def klen(str s) -> int64:
    return len(s)

def main():
    # Iterable min/max over an int list (exact integer compare).
    auto xs = [5, 2, 8, 1, 9]
    print(min(xs))  # 1
    print(max(xs))  # 9

    # Over a str list (lexicographic).
    auto ws = ["banana", "apple", "cherry"]
    print(min(ws))  # apple
    print(max(ws))  # cherry

    # key= projects each element to an orderable key; ties keep the first.
    print(min(ws, key=klen))  # apple
    print(max(ws, key=klen))  # banana

    # default= is returned for an empty list.
    print(min(list[int64](), default=-1))  # -1
    print(max(list[int64](), default=-1))  # -1

    # sum(xs, start) seeds the accumulator; stays exact for ints.
    print(sum([1, 2, 3], 100))  # 106
    print(sum([5], 100))  # 105

    # enumerate: index + element.
    for i, x in enumerate([10, 20, 30]):
        print(i)  # 0, 1, 2
        print(x)  # 10, 20, 30

    # enumerate with an explicit start.
    for j, y in enumerate([7, 8], 1):
        print(j)  # 1, 2

    # zip truncates to the shortest input.
    for a, b in zip([1, 2, 3], [100, 200]):
        print(a + b)  # 101, 202
