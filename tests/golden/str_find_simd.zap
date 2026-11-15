# Golden fixture: substring / byte search through the SIMD memchr/memmem kernel
# (#945, ISA-acceleration epic #139). `str.find` / `in` route through the
# multiversioned `zap_memmem_off` (first-byte SIMD memchr + memcmp verify). The
# string is long enough that a tail match lands past the 32-byte AVX2 block, so
# the vectorized block loop and the scalar tail are both exercised. Whichever
# clone the host selects, the result is identical — arch/CPU-independent, so this
# output is the same on every CI box. (The `str_find_simd_generic` sibling forces
# the baseline clone via ZAP_CPU=generic and prints the same thing.)
def main():
    auto s = "the quick brown fox jumps over the lazy dog, then the fox naps away"
    print(s.find("the"))  # 0  (first occurrence)
    print(s.find("fox"))  # 16 (first occurrence, not the later one)
    print(s.find(" "))  # 3  (single-byte needle -> memchr fast path)
    print(s.find("missing"))  # -1 (absent)
    print("lazy" in s)  # True
    print("zebra" in s)  # False
    if s.find("dog") > 32:  # a match in a later SIMD block
        print("dog_in_tail")
