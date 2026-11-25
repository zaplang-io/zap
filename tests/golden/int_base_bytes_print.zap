# Golden fixture: overload-parity batch A (#1458) — int(s, base), bytes(int),
# and print() formatting kwargs (sep=/end=/file=/flush=).
import fs
import os
import io


def main():
    # --- int(s, base): base 2..=36, or base 0 to auto-detect a prefix ---
    print(int("ff", 16))  # 255
    print(int("0b1010", 0))  # 10  (prefix auto-detected)
    print(int("777", 8))  # 511
    print(int("z", 36))  # 35
    print(int("  101  ", 2))  # 5   (surrounding whitespace, binary 101)
    print(int("0xFF", 16))  # 255 (explicit prefix accepted in base 16)
    print(int("-10", 2))  # -2
    print(int("42", 0))  # 42  (base 0, no prefix -> decimal)
    try:
        auto _a = int("", 10)
    except ValueError:
        print("int-empty:valueerror")
    try:
        auto _b = int("2", 2)
    except ValueError:
        print("int-baddigit:valueerror")
    try:
        auto _c = int("10", 37)
    except ValueError:
        print("int-badbase:valueerror")

    # --- bytes(int): n zero bytes; negative -> ValueError ---
    print(len(bytes(0)))  # 0
    print(len(bytes(4)))  # 4
    try:
        auto _z = bytes(-1)
    except ValueError:
        print("bytes-neg:valueerror")

    # --- print(sep=, end=, flush=) ---
    print("a", "b", sep=", ")  # a, b
    print("x", end="")
    print("y")  # xy  (previous print wrote no newline)
    print("a", "b", "c", sep="-", end="!\n")  # a-b-c!
    print(end="end-only\n")  # end-only  (no positionals, custom end)
    print(1, 2, 3, sep="")  # 123
    print("flushed", flush=True)  # flushed  (flush is a no-op here)

    # --- print(file=fd): write to an int64 fd, read it back ---
    auto d = fs.mkdtemp()
    auto f = os.path_join(d, "p.txt")
    auto w = open(f, "w")
    print("to-file", end="", file=w)
    io.close_fd(w)
    auto r = open(f)
    print("file:" + io.read(r, 100))  # file:to-file
    io.close_fd(r)
    fs.rmtree(d)
