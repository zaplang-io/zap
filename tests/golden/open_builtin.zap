# Golden fixture: the open() prelude built-in (#1388) — thin sugar over io.open.
# open(path) defaults mode "r"; open(path, mode) passes the mode through. The
# handle is the same int64 fd io.open returns (usable with io.read/write/
# close_fd), and the failure behaviour is identical: a missing path raises
# FileNotFoundError (catchable as OSError / Exception), a bad mode ValueError.
# Only fixed labels / lengths are printed, never the non-deterministic temp path.
import os
import fs
import io


def main():
    auto d = fs.mkdtemp()
    auto f = os.path_join(d, "greet.txt")

    # open(path, "w") == io.open(path, "w"): write then close.
    auto w = open(f, "w")
    print(io.write(w, "helloworld"))  # 10
    io.close_fd(w)

    # open(path) defaults to mode "r".
    auto r = open(f)
    print(io.read(r, 5))  # hello
    print(io.read(r, 100))  # world
    io.close_fd(r)

    # open(path, "r") explicit mode: same handle, reads the whole file.
    auto r2 = open(f, "r")
    print(io.read(r2, 100))  # helloworld
    io.close_fd(r2)

    # A missing path raises FileNotFoundError (catchable at every level).
    auto missing = os.path_join(d, "nope.txt")
    try:
        auto _bad = open(missing)
    except FileNotFoundError:
        print("open-missing:filenotfound")
    try:
        auto _bad2 = open(missing)
    except Exception:
        print("open-missing:exception")

    # A bad mode is a caller-input error -> ValueError.
    try:
        auto _bad3 = open(f, "zzz")
    except ValueError:
        print("open-badmode:valueerror")

    fs.rmtree(d)
