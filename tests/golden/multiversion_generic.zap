# Golden fixture: the ZAP_CPU=generic downgrade forces a @multiversion function
# onto its mandatory `default` (baseline) clone on any hardware (#944 / #943).
# The sidecar `.env` sets ZAP_CPU=generic, so cpu_features reports baseline-only
# and the resolver must pick `default`. The output is identical to
# `multiversion_basic` — the whole point: the fallback clone computes the same
# result, and a feature-absent host never traps (no SIGILL).
@multiversion("avx2", "sse4.2", "default")
def sum_sq(int64 n) -> int64:
    int64 acc = 0
    for i in range(0, n):
        acc = acc + i * i
    return acc

def main():
    print(sum_sq(0))
    print(sum_sq(1))
    print(sum_sq(5))
    print(sum_sq(10))
    print(sum_sq(100))
