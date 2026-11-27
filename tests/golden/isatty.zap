# Golden fixture: io.isatty terminal detection (#1685). Under the test harness
# stdout/stderr are captured pipes and a freshly opened regular file is not a
# terminal, so every query here is deterministically False; the PTY "True" case
# lives in a driver test (crates/driver/tests/zap_cli.rs). Proves the query
# returns a usable bool, never raises, and that an invalid descriptor folds to
# False (CPython os.isatty parity). Cleans up its temp dir.
import io
import os
import fs
import sys


def main():
    # Captured standard streams are pipes, not terminals.
    print(io.isatty(sys.stdout_fd()))  # False
    print(io.isatty(sys.stderr_fd()))  # False

    # A regular file is never a terminal.
    auto d = fs.mkdtemp()
    auto path = os.path_join(d, "f.txt")
    fs.write_file(path, "x")
    auto fd = io.open(path, "r")
    print(io.isatty(fd))  # False
    io.close_fd(fd)

    # An invalid / out-of-range descriptor folds to False, never raising.
    print(io.isatty(4242))  # False

    # The result is a usable bool; the keyword form binds `fd` too.
    if not io.isatty(fd=sys.stdout_fd()):
        print("stdout-not-a-tty")

    fs.rmtree(d)
