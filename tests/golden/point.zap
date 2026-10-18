# Golden fixture: structs — fields, methods, construction, field get/set.
struct Point:
    f64 x
    f64 y

    def sumsq(self) -> f64:
        return self.x * self.x + self.y * self.y

def main():
    auto p = Point(x=3.0, y=4.0)
    print(p.sumsq())
    p.x = 6.0
    print(p.x)
