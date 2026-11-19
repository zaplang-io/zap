# Golden fixture: io buffered writer / file object (#1071, D-0240) — the write
# twin of the #969 buffered reader. Exercises write/flush/close_ over a fresh
# mkdtemp sandbox, a round-trip read-back (fs.read_file and the #969
# io.reader/io.readall), a large write that bypasses the 8 KiB buffer, ordered
# buffered+large writes, the `with io.writer(fd) as w:` auto-close form (on
# normal exit and on unwind), and the catchable negatives (write-after-close,
# bad fd). Output is independent of the temp path, so it is deterministic across
# collectors and OSes.
import io
import os
import fs


# Build `unit` repeated `n` times (no str-repeat operator relied on).
def repeat(str unit, int64 n) -> str:
    auto out = ""
    for i in range(0, n):
        out = out + unit
    return out


def main():
    auto d = fs.mkdtemp()

    # write -> flush -> close_, returning the byte count of each write.
    auto f1 = os.path_join(d, "out.txt")
    auto w = io.writer(io.open(f1, "w"))
    print(w.write("hello\n"))  # 6
    print(w.write("world"))  # 5
    w.flush()
    w.close_()

    # round-trip: read the whole file back with fs.read_file ...
    print(fs.read_file(f1))  # hello / world
    # ... and with the #969 buffered reader.
    auto r = io.reader(io.open(f1, "r"))
    print(io.readall(r))  # hello / world

    # large write (>= 8 KiB) takes the buffer-bypass path; byte count and file
    # size round-trip exactly.
    auto big = repeat(repeat("x", 100), 100)  # 10000 bytes
    auto f2 = os.path_join(d, "big.bin")
    auto w2 = io.writer(io.open(f2, "w"))
    print(w2.write(big))  # 10000
    w2.close_()
    print(fs.size(f2))  # 10000

    # mixed small-buffered + large writes keep output ordered end to end.
    auto f3 = os.path_join(d, "mixed.txt")
    auto w3 = io.writer(io.open(f3, "w"))
    w3.write("head:")
    w3.write(repeat(repeat("y", 100), 90))  # 9000 bytes, crosses the buffer
    w3.write(":tail")
    w3.close_()
    auto c3 = fs.read_file(f3)
    print(len(c3))  # 9010
    print(c3[0:5])  # head:
    print(c3[9005:9010])  # :tail

    # `with io.writer(fd) as w:` auto-closes (flush + close) on scope exit.
    auto f5 = os.path_join(d, "with.txt")
    with io.writer(io.open(f5, "w")) as ww:
        ww.write("one\n")
        ww.write("two")
    print(fs.read_file(f5))  # one / two

    # __exit__ runs even on unwind: the partial write is flushed before the
    # exception leaves the `with`.
    auto f6 = os.path_join(d, "unwind.txt")
    try:
        with io.writer(io.open(f6, "w")) as wu:
            wu.write("partial")
            raise "boom"
    except Exception:
        print("unwound")
    print(fs.read_file(f6))  # partial

    # write after close_ raises a catchable ValueError with a fixed message.
    auto f7 = os.path_join(d, "closed.txt")
    auto w7 = io.writer(io.open(f7, "w"))
    w7.write("x")
    w7.close_()
    try:
        w7.write("more")
    except Exception as e:
        print(e)  # I/O operation on closed writer
    # close_ is idempotent — a second close_ is a no-op (never raises).
    w7.close_()
    print("double-close ok")

    # a bad fd surfaces a catchable I/O error once the buffer is flushed.
    auto wbad = io.writer(-1)
    try:
        wbad.write("data")
        wbad.flush()
    except Exception:
        print("bad fd caught")

    fs.rmtree(d)
