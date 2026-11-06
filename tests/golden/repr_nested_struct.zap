# Golden fixture: the default renderer recurses through user-struct fields by
# CALLING that field type's own renderer (#274, D-0124), so a struct of structs
# renders fully without any opt-in. A mutually/recursively referenced type
# terminates at compile time because each struct generates one renderer function.
struct Point:
    int64 x
    int64 y

struct Line:
    Point a
    Point b

def main():
    auto l = Line(a=Point(x=1, y=2), b=Point(x=3, y=4))
    print(l)  # Line(a=Point(x=1, y=2), b=Point(x=3, y=4))
