# Golden fixture: str(bytes, encoding[, errors]) decode (#1459).
# Encodings utf-8 / ascii / latin-1; errors strict (default) / replace / ignore.
# Only ASCII labels / codepoint numbers / lengths are printed (never a raw
# non-ASCII character), so the expected file stays ASCII-clean.
def main():
    # --- utf-8 ---
    print(str(bytes([104, 105]), "utf-8"))  # hi
    auto e = str(bytes([195, 169]), "utf-8")  # "é" (U+00E9), 2 UTF-8 bytes
    print(len(e))  # 1   (one codepoint)
    print(ord(e[0]))  # 233
    try:
        auto _a = str(bytes([255]), "utf-8")
    except ValueError:
        print("utf8-strict:valueerror")
    print(len(str(bytes([255]), "utf-8", "replace")))  # 1  (U+FFFD)
    print(len(str(bytes([255]), "utf-8", "ignore")))  # 0  (dropped)
    print(ord(str(bytes([255]), "utf-8", "replace")[0]))  # 65533 (U+FFFD)

    # --- ascii ---
    print(str(bytes([104, 105]), "ascii"))  # hi
    try:
        auto _b = str(bytes([200]), "ascii")
    except ValueError:
        print("ascii-strict:valueerror")
    print(len(str(bytes([200]), "ascii", "ignore")))  # 0
    print(ord(str(bytes([200]), "ascii", "replace")[0]))  # 65533

    # --- latin-1 (never fails; byte b -> U+00bb) ---
    print(ord(str(bytes([233]), "latin-1")[0]))  # 233
    print(ord(str(bytes([255]), "iso-8859-1")[0]))  # 255

    # --- unknown encoding / errors handler -> ValueError ---
    try:
        auto _c = str(bytes([104]), "cp1252")
    except ValueError:
        print("unknown-encoding:valueerror")
    try:
        auto _d = str(bytes([255]), "utf-8", "boom")
    except ValueError:
        print("unknown-errors:valueerror")
