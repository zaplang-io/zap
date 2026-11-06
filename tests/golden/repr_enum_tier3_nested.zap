# Golden fixture: a tier-3 enum's payload fields render through the ONE value
# renderer in repr mode (#274/#706, D-0124), so nested values render exactly as
# they would inside a struct/container: a `str`/`char` payload is Python-quoted
# (`'hi'`, `'z'`), a `bool` renders `True`/`False`, a nested struct renders its
# own `TypeName(field=value)` form, and a tuple renders the `(v0, v1)` form with
# its own elements quoted — all reached by *calling* each type's renderer.
struct Point:
    int64 x
    int64 y


enum Node:
    Leaf(str name, char tag)
    Flag(bool on, int64 n)
    Pair(Point p, (int64, str) t)
    Empty


def main():
    print(Node.Leaf(name="hi", tag=char(122)))  # Node.Leaf(name='hi', tag='z')
    print(Node.Flag(on=True, n=3))  # Node.Flag(on=True, n=3)
    print(Node.Pair(Point(x=1, y=2), (7, "ok")))  # Node.Pair(p=Point(x=1, y=2), t=(7, 'ok'))
    print(Node.Empty)  # Node.Empty
    print(f"{Node.Leaf(name='q', tag=char(97))}")  # Node.Leaf(name='q', tag='a')
