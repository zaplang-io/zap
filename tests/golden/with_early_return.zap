# Golden (I#185): an early `return` inside a `with` must still run `__exit__`
# (the motivating lock-release case). `f` returns from inside `with Guard():`;
# the cleanup stack walked by `return` runs `__exit__` (prints "exit") before the
# function returns 42. Proves structured early-exit cleanup for `with`.
struct Guard:
    int64 id
    def __enter__(self) -> Guard:
        print("enter")
        return self
    def __exit__(self):
        print("exit")

def f() -> int64:
    with Guard(1):
        print("in with")
        return 42
    return 0

def main() -> int64:
    print(f())
    return 0
