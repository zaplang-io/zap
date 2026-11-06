# Golden fixture: tuples render the Python (v0, v1, ...) form via the general
# Display protocol (#274, D-0124). A one-element tuple keeps its trailing comma
# `(v0,)` (D-0049). Elements render in repr mode, so a string element is QUOTED
# ('k') while a bare top-level string printed on its own stays UNQUOTED. Nested
# tuples and tuples of structs render recursively; str()/f-string match print().
struct Point:
    int64 x
    int64 y

def main():
    print((1, 2))  # (1, 2)
    print((9,))  # (9,)  single-element keeps trailing comma
    print((1, (2, 3)))  # (1, (2, 3))  nested
    print((Point(x=1, y=2), "k"))  # (Point(x=1, y=2), 'k')  str element quoted
    auto t = (True, 3.5)
    print(str(t))  # (True, 3.5)
    print(f"{t}")  # (True, 3.5)
    print("bare")  # bare   top-level string stays unquoted
