# diff: match
# str(bytes, encoding[, errors]) decode matches CPython (#1459) for the
# successful, deterministic cases: utf-8 round-trip and replace/ignore counts,
# ascii replace/ignore, and latin-1 byte->codepoint. Only codepoint numbers and
# lengths are printed. The error cases diverge by exception *type* (Zap raises
# ValueError; CPython raises UnicodeDecodeError for a bad decode and LookupError
# for an unknown encoding/errors handler), so they live in the golden suite.
def main():
    print(str(bytes([104, 105]), "utf-8"))
    print(ord(str(bytes([195, 169]), "utf-8")[0]))
    print(len(str(bytes([255]), "utf-8", "replace")))
    print(len(str(bytes([255]), "utf-8", "ignore")))
    print(ord(str(bytes([255]), "utf-8", "replace")[0]))
    print(str(bytes([104, 105]), "ascii"))
    print(len(str(bytes([200]), "ascii", "ignore")))
    print(ord(str(bytes([200]), "ascii", "replace")[0]))
    print(ord(str(bytes([233]), "latin-1")[0]))
    print(ord(str(bytes([255]), "iso-8859-1")[0]))
