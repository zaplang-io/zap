# Golden fixture: the same substring search as `str_find_simd`, but the sidecar
# `.env` sets ZAP_CPU=generic so the memchr resolver is forced onto its baseline
# clone (scalar / SSE2 floor / NEON floor) on any hardware (#945 / #943). The
# result is identical — proving the baseline search path is correct and a
# feature-absent host never traps.
def main():
    auto s = "the quick brown fox jumps over the lazy dog, then the fox naps away"
    print(s.find("the"))
    print(s.find("fox"))
    print(s.find(" "))
    print(s.find("missing"))
    print("lazy" in s)
    print("zebra" in s)
    if s.find("dog") > 32:
        print("dog_in_tail")
