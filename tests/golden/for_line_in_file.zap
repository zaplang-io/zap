# Golden fixture: `for line in file` over the prelude LineReader (#1031, D-0223).
# A buffered reader participates in the D-0048 iterator protocol via io.lines /
# fs.open_reader, so the Python-natural `for line in open(path)` works. Each
# yielded line KEEPS its trailing '\n' (we strip it for printing); a blank line
# is preserved (reads as "\n"), and only true EOF ends the loop. Exercises:
# multi-line incl. a final unterminated line, a preserved blank line, both entry
# points, early break, an empty file (zero iterations), and nested iteration
# with independent readers. Deterministic — no temp path is printed. Cleans up.
import os
import fs
import io


def main():
    auto d = fs.mkdtemp()
    auto path = os.path_join(d, "data.txt")
    # Three lines: a blank middle line, and a final line with NO trailing newline.
    fs.write_file(path, "alpha\n\ngamma")

    # 1) for line in fs.open_reader(path): lines keep '\n'; strip for printing.
    auto n = 0
    for line in fs.open_reader(path):
        n = n + 1
        print(line.removesuffix("\n"))
    print(n)  # 3 — the blank middle line is preserved, not read as EOF

    # 2) for line in io.lines(io.reader(fd)): the io-level entry over the same file.
    auto fd = io.open(path, "r")
    for line in io.lines(io.reader(fd)):
        print(line.removesuffix("\n"))
    io.close_fd(fd)

    # 3) early break: stop after the first line.
    for line in fs.open_reader(path):
        print("first=" + line.removesuffix("\n"))
        break

    # 4) empty file yields zero iterations.
    auto empty = os.path_join(d, "empty.txt")
    fs.write_file(empty, "")
    auto m = 0
    for line in fs.open_reader(empty):
        m = m + 1
    print(m)  # 0

    # 5) nested iteration with independent readers: 3 outer lines x 2 inner = 6.
    auto pair = os.path_join(d, "pair.txt")
    fs.write_file(pair, "x\ny")
    auto prod = 0
    for line in fs.open_reader(path):
        for inner in fs.open_reader(pair):
            prod = prod + 1
    print(prod)  # 6

    fs.rmtree(d)
