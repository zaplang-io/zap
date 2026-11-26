# Golden fixture: the shared string scratch-buffer helper grows correctly across
# its three public callers (#1599). `zap_strbuf_ensure` (backing str.expandtabs /
# str.translate / str.format_map) previously assigned realloc's result straight
# back into its buffer before the NULL check, leaking the old allocation on a
# growth failure. The leak itself cannot be driven on a healthy host (it needs an
# injected allocation fault — pinned by the host source-contract test
# `strbuf_ensure_frees_old_buffer_before_raising_1599`). What this fixture locks
# is that the growth path — the exact code the fix touches — still produces the
# right bytes: each caller is fed an input whose result is far larger than the
# helper's 16-byte initial capacity, so the realloc loop runs several times.
# Intent documentation / regression wall (acceptance criterion: the three public
# callers still return the same strings on successful allocation).

def main():
    # --- str.expandtabs: 40 copies of "ab\t" -> 160 chars (grows 16->256) ----
    auto s = ""
    for i in range(0, 40):
        s = s + "ab\t"
    auto e = s.expandtabs(4)
    # Each "ab\t" expands to "ab" + 2 spaces (tab stop every 4 cols): 4 chars.
    # Slices end on a letter so no `.expected` line carries trailing whitespace.
    print(len(e))  # 160
    print(e[:6])  # "ab  ab"

    # --- str.translate: 50 copies of "abc" -> each char to 3 chars -> 450 -----
    auto s2 = ""
    for i in range(0, 50):
        s2 = s2 + "abc"
    auto t = {"a": "AAA", "b": "BBB", "c": "CCC"}
    auto tr = s2.translate(t)
    print(len(tr))  # 450
    print(tr[:9])  # "AAABBBCCC"

    # --- str.format_map: "{x}" repeated 30 times, value is 10 chars -> 300 ----
    auto tmpl = ""
    for i in range(0, 30):
        tmpl = tmpl + "{x}"
    auto m = {"x": "0123456789"}
    auto fm = tmpl.format_map(m)
    print(len(fm))  # 300
    print(fm[:10])  # "0123456789"

    # The growth path preserves exact content, not just length: the tail bytes
    # are correct too (catches an off-by-one or a lost tail on a realloc move).
    print(e[156:158])  # "ab"
    print(tr[441:])  # "AAABBBCCC"

    print("strbuf_growth_1599: PASS")
