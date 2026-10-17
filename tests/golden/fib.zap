# Golden fixture: recursive Fibonacci (Phase 1 milestone program).
def fib(int64 n) -> int64:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def main():
    print(fib(10))
