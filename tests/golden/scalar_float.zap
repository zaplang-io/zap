# Golden fixture: the scalar `float` type (I#569), lowered as a 64-bit double.
# `float` is the Python-facing name for `f64`: it works as a binding, a function
# parameter and return, in arithmetic (`+ - * /`, true division — no integer
# flooring) and comparison, and an int value widens into it (D-0020). Values are
# chosen to format exactly (no platform-sensitive rounding).
def half(float x) -> float:
    return x / 2.0

def main():
    float a = 1.5
    float b = 0.5
    print(a + b)  # 2.0
    print(a - b)  # 1.0
    print(a * b)  # 0.75
    print(a / b)  # 3.0  (true division)
    print(a > b)  # True
    print(a == 1.5)  # True
    print(a < b)  # False
    print(half(3.0))  # 1.5
    float c = 3  # an int literal widens into a float
    print(c)  # 3.0
    print(c + 1)  # 4.0  (mixed int/float arithmetic is float)
