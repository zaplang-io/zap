# Golden fixture: bulk `bytes.translate(table[, delete])` + `bytes.maketrans`
# (ticket #1099). Byte-table translation in one native pass, Python parity:
# identity tables, a DNA-complement table, the optional delete set
# (deletion-wins over remapping), empty input, non-ASCII (0x80..0xFF) bytes, and
# the maketrans/translate round-trip. The table-length and maketrans
# equal-length faults are catchable and covered by bytes_translate_reject.zap;
# str.translate(dict) regression lives in str_translate.zap.
def main():
    # --- identity table: output == input --------------------------------
    auto ident = bytes.maketrans(b"", b"")
    print(str(b"hello".translate(ident)))  # hello
    print(len(ident))  # 256

    # --- single-byte remap ----------------------------------------------
    auto t = bytes.maketrans(b"ab", b"AB")
    print(str(b"abcab".translate(t)))  # ABcAB
    print(str(b"zzz".translate(t)))  # zzz  (no source byte matches)

    # --- DNA complement (the issue's reverse-complement leg) ------------
    auto comp = bytes.maketrans(b"ACGTacgt", b"TGCATGCA")
    print(str(b"ACGTacgt".translate(comp)))  # TGCATGCA
    print(str(b"GATTACA".translate(comp)))  # CTAATGT

    # --- empty input -> empty output ------------------------------------
    print(len(b"".translate(comp)))  # 0

    # --- delete set: non-overlapping with the table ---------------------
    print(str(b"a\nb\nc".translate(ident, b"\n")))  # abc

    # --- delete set overlapping the remapped bytes: deletion WINS -------
    # `a` is remapped to `A` by `t`, but also in the delete set, so it is
    # dropped rather than translated (CPython parity).
    print(str(b"abcab".translate(t, b"a")))  # Bc B  -> "BcB"
    print(str(b"abcab".translate(t, b"a") == b"BcB"))  # True

    # --- empty delete set behaves like no delete set --------------------
    print(str(b"abcab".translate(t, b"") == b"abcab".translate(t)))  # True

    # --- non-ASCII (0x80..0xFF) bytes, unsigned, no sign extension ------
    auto hi = bytes.maketrans(bytes([0x80, 0xFF]), bytes([0x01, 0x02]))
    auto out = bytes([0x80, 0x00, 0xFF]).translate(hi)
    print(out[0])  # 1
    print(out[1])  # 0
    print(out[2])  # 2

    # --- maketrans + translate round-trip: table then its inverse = id --
    auto fwd = bytes.maketrans(b"ACGT", b"TGCA")
    auto inv = bytes.maketrans(b"TGCA", b"ACGT")
    auto seq = b"ACGTACGTAC"
    print(str(seq.translate(fwd).translate(inv) == seq))  # True
