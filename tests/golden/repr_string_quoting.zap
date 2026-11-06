# Golden fixture: the Python str/repr distinction (#274, D-0124). A bare string
# printed at the top level is UNQUOTED; the same string as a struct field or a
# container element is QUOTED (repr). bytes render as the Python b'...' form when
# nested. This pins the behavior change: a str field in the default render /
# @derive(Repr) now shows quotes.
struct Rec:
    str name
    bytes blob

def main():
    auto r = Rec(name="ada", blob=bytes("hi"))
    print(r)  # Rec(name='ada', blob=b'hi')
    print("ada")  # ada   bare top-level string stays unquoted
    print(str("ada"))  # ada
    print(["ada"])  # ['ada']  element quoted
    print(f"{r}")  # Rec(name='ada', blob=b'hi')
