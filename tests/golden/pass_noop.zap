# Golden fixture: `pass` as a general no-op statement (#591). `pass` is accepted
# in every statement context and as the empty struct/enum body spelling, and
# lowers to nothing (no IR). This program runs each context and prints a running
# total so the no-ops are observably inert.
struct Empty:
    pass

enum Nothing:
    pass

def classify(int64 n) -> int64:
    if n > 0:
        pass
    else:
        pass
    return n

def main():
    int64 total = 0
    for i in range(3):
        pass
    total = total + 1
    int64 k = 0
    while k < 2:
        pass
        k = k + 1
    total = total + 1
    try:
        pass
    finally:
        pass
    total = total + 1
    scope:
        pass
    total = total + 1
    print(total)
    print(classify(5))
