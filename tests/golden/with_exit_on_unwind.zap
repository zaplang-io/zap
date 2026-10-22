# Golden (I#185): `__exit__` must run while an exception unwinds OUT of the with
# body, and the exception must still propagate. `risky` raises inside `with
# Guard():`; the guard's `__exit__` runs during unwind (prints "exit"), then the
# exception reaches main's handler. "after raise" is never printed. Proves the
# per-with landing pad runs cleanup then re-raises (chaining to the caller).
struct Guard:
    int64 id
    def __enter__(self) -> Guard:
        print("enter")
        return self
    def __exit__(self):
        print("exit")

def risky():
    with Guard(1):
        print("before raise")
        raise "boom"
        print("after raise")

def main():
    try:
        risky()
    except Exception as e:
        print("caught:")
        print(e)
