# Golden fixture: variadic tuple[T] element-kind coverage (#1476) — enum and
# bytes elements, a tuple[T] nested inside a list (read-back recovers the precise
# tuple type and renders it `(…)`), and an enum-element tuple as a set member.
enum Color:
    RED
    GREEN
    BLUE


def main():
    # enum-element tuple
    auto e = tuple([Color.RED, Color.BLUE])
    print(e)  # (Color.RED, Color.BLUE)
    print(e[1])  # Color.BLUE
    print(e == tuple([Color.RED, Color.BLUE]))  # True

    # bytes-element tuple (elements compared by content)
    auto b = tuple([bytes("hi"), bytes("yo")])
    print(b)  # (b'hi', b'yo')
    print(b == tuple([bytes("hi"), bytes("yo")]))  # True

    # a tuple[T] nested in a list: read-back recovers tuple[int64] and renders (…)
    auto nested = [tuple([1, 2]), tuple([3, 4])]
    print(nested)  # [(1, 2), (3, 4)]
    print(nested[0])  # (1, 2)
    print(nested[1][1])  # 4

    # enum-element tuple as a set member (dedup by content)
    auto s = set([tuple([Color.RED]), tuple([Color.BLUE]), tuple([Color.RED])])
    print(len(s))  # 2
