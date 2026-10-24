"""Shape utilities.

A tiny module used to exercise ``zap doc`` rendering. It covers a
constant, functions, a struct with fields and methods, an enum, and an
interface — the full item surface.
"""

const float64 PI = 3.14159


@memoize
def area(float64 r) -> float64:
    """Area of a circle.

    Args:
        r: the radius, in metres

    Returns:
        the area enclosed by the circle
    """
    return PI * r * r


def _scale(float64 r, float64 k) -> float64:
    return r * k


def clamp[T: Ordered](T x, T lo, T hi) -> T:
    "Clamp ``x`` into the inclusive range ``[lo, hi]``."
    return x


struct Point:
    float64 x
    float64 y
    int32 _tag

    def dist(Point other) -> float64:
        "Euclidean distance to `other`."
        auto dx = self.x - other.x
        return dx


enum Shape:
    Circle(float64 r)
    Square(float64 side)

    def describe(self) -> str:
        "Human label for the shape."
        return "shape"


interface Drawable:
    def draw(self) -> str
