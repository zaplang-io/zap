# A library module exporting a struct and functions over it.
struct Point:
    int64 x
    int64 y

def make(int64 a, int64 b) -> Point:
    return Point(x=a, y=b)

def manhattan(Point p) -> int64:
    return p.x + p.y
