# Golden fixture + HARD INVARIANT (#709, #702/#705/D-0122): making `char` a
# distinct IR type must NOT change how a char is hashed or compared as a dict
# key / set element. A char key is still stored and probed through the `_i64`
# runtime entry points (`zap_dict_set_i64_i64` / `zap_dict_contains_i64` /
# `zap_set_add_i64`), byte-identical to the pre-#709 path where `char` was
# `Ty::I32`. This round-trips char keys built one way (string iteration) and
# probed another (`char()`, `chr()`, `s[i]`): if the hash/equality path had
# shifted, these lookups would miss. Only lookups and `len` are printed, never a
# whole multi-entry dict/set, so the golden is independent of hash iteration
# order.
def main():
    auto s = "aba"
    dict[char, int64] counts = {}
    for c in s:
        counts[c] = counts.get(c, 0) + 1
    # Keys inserted as the loop variable `c`; probed as `char(n)` — same key.
    print(counts[char(97)])  # 2
    print(counts[char(98)])  # 1
    print(char(97) in counts)  # True
    print(char(122) in counts)  # False
    print(len(counts))  # 2

    set[char] seen = set[char]()
    for c in s:
        seen.add(c)
    print(char(98) in seen)  # True
    print(chr(97) in seen)  # True
    print(char(122) in seen)  # False
    print(len(seen))  # 2

    # Cross-source key equality: a key inserted as `s[0]` is found by `chr(97)`.
    dict[char, str] m = {}
    m[s[0]] = "first"
    print(m[chr(97)])  # first
