# diff: match
# Positional call arguments are evaluated left-to-right before the call, matching
# CPython (epic #374, walled garden): `f(g(), h())` evaluates g() then h(), so
# "g" prints before "h", and the resulting values reach the parameters in order
# (a=1, b=2 -> 12). Side-effecting argument functions pin the order.
def g() -> int64:
    print("g")
    return 1


def h() -> int64:
    print("h")
    return 2


def f(int64 a, int64 b) -> int64:
    return a * 10 + b


def main():
    print(f(g(), h()))
