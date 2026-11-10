# Golden fixture: a tier-3 (data-carrying) enum is a tagged value aggregate that
# AUTO-derives Eq + Hash under the capability model (D-0139/#809), so it works as
# a dict key with no `@derive`. Keys compare by tag then payload: distinct tags or
# distinct payloads are distinct keys; the same tag+payload collides.
enum Tag:
    A(int64 x)
    B(str y)


def main():
    dict[Tag, int64] d = {}
    d[Tag.A(x=1)] = 10
    d[Tag.B(y="hi")] = 20
    print(d[Tag.A(x=1)])  # 10
    print(d[Tag.B(y="hi")])  # 20
    print(len(d))  # 2

    # A different payload is a different key.
    d[Tag.A(x=2)] = 30
    print(len(d))  # 3

    # Same tag + payload collides to the one slot (overwrite).
    d[Tag.A(x=1)] = 99
    print(d[Tag.A(x=1)])  # 99
    print(len(d))  # 3
