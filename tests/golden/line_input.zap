# Golden fixture: line-oriented input (ticket #969, D-0212). Exercises the
# buffered reader (io.reader / readline / readall / readn) and the fs.lines
# convenience against a fresh mkdtemp sandbox, printing only deterministic
# results. The `input()` builtin reads stdin and is covered by a driver test
# (crates/driver/tests), not here. Cleans up after itself with rmtree.
import os
import fs
import io


def main():
    auto d = fs.mkdtemp()
    auto path = os.path_join(d, "data.txt")
    # Three lines; the last has NO trailing newline, to prove readline handles a
    # final unterminated line and that "" (not a blank line) marks EOF.
    fs.write_file(path, "alpha\nbeta\ngamma")

    # Streaming read-ahead loop. readline keeps the trailing '\n', so a blank
    # line would read as "\n" and only true EOF reads as "".
    auto fd = io.open(path, "r")
    auto r = io.reader(fd)
    auto n = 0
    auto line = io.readline(r)
    while line != "":
        n = n + 1
        print(line.removesuffix("\n"))
        line = io.readline(r)
    io.close_fd(fd)
    print(n)  # 3

    # readn then readall over a fresh reader: 5 bytes, then the remainder.
    auto fd2 = io.open(path, "r")
    auto r2 = io.reader(fd2)
    print(io.readn(r2, 5))  # alpha
    print(len(io.readall(r2)))  # 11  = len("\nbeta\ngamma")
    # A drained reader yields "" / 0-length reads.
    print(len(io.readline(r2)))  # 0
    print(len(io.readn(r2, 4)))  # 0
    io.close_fd(fd2)

    # fs.lines: whole-file read split into lines, terminators stripped.
    auto ls = fs.lines(path)
    print(len(ls))  # 3
    for i in range(0, len(ls)):
        print(ls[i])

    fs.rmtree(d)
