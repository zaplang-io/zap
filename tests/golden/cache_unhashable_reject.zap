# @cache (#666) requires hashable parameters: a `list` cannot be a dict key, so
# this signature is rejected at compile time with a stable coded diagnostic
# (ZAP5056) — never an internal compiler error. Compile-fail fixture.
@cache
def f(list[int64] xs) -> int64:
    return len(xs)

def main():
    print(f([1, 2, 3]))
