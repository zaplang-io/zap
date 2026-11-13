# Dual-collector soundness (#553): arena objects are pinned (non-moving) and are
# NOT reclaimed by the collector — they are freed only at block exit. After
# churning the GC heap and forcing collections mid-block, the arena contents must
# still read back correctly. Contents only — no heap-byte assertions (those are
# non-deterministic across collectors).
struct Rec:
    int64 v

def churn(int64 n):
    for i in range(0, n):
        auto g = [i, i + 1, i + 2]

def main():
    with arena(size=4096) as a:
        auto r = a.new(Rec, 42)
        auto buf = a.alloc(bytes, 128)
        churn(200000)
        gc.collect()
        gc.collect()
        print(r.v)
        print(len(buf))
