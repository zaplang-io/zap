# Borrowed-bytes GC soundness (#1082): a strview held across a GC safepoint must
# survive a moving-Immix evacuation of its backing. The view carries its backing
# as a traced INTERIOR pointer (the "2b" base+offset representation, reusing the
# #586 aggregate-root (base,desc) pairs), so the collector rewrites the base on a
# move and the borrowed bytes still read back after churn + forced collections.
# Contents only — no heap-byte assertions (non-deterministic across collectors).
def churn(int64 n):
    for i in range(0, n):
        auto g = [i, i + 1, i + 2]

def main():
    str s = "borrowed-bytes-survive-eviction"
    strview v = s.view(0, 8)  # "borrowed"
    churn(200000)
    gc.collect()
    gc.collect()
    print(v == "borrowed")  # True
    print(len(v))  # 8
    print(v.startswith("borrow"))  # True
    print(v.to_str())  # borrowed
