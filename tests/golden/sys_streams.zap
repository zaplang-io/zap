# Golden fixture: sys.stdout() stream object + sys.write (ticket #1035, D-0217).
# Writes go straight to fd 1, so the output is exactly the written bytes in
# order (no print interleaving). stdin reading and stderr are covered by driver
# tests (they need a piped stdin / a separate stderr capture the golden harness
# does not provide).
import sys


def main():
    auto out = sys.stdout()
    sys.write(out, "hello\n")
    sys.write(out, "world\n")
    auto n = sys.write(out, "abcde")
    sys.write(out, "\n")
    if n == 5:
        sys.write(out, "wrote-5\n")
