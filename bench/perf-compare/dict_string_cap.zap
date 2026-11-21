def main():
    # Same workload as dict_string.zap but the dict is pre-sized to the known
    # final size via the #1083 capacity hint, so the build pays no grow+rehash
    # cycles. Zap-only: the Go/Python companions are byte-identical to the
    # dict_string ones, so the ratio columns isolate the Zap capacity effect.
    dict[str, int64] d = dict[str, int64](capacity=200000)
    int64 i = 0
    while i < 200000:
        d[f"k{i}"] = i
        i = i + 1
    int64 s = 0
    i = 0
    while i < 200000:
        s = s + d[f"k{i}"]
        i = i + 1
    print(s)
