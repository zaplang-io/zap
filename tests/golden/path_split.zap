# Golden fixture: os.path_split / os.path_splitext (ticket #1030, D-0219). Pure
# lexical path decomposition returning a (str, str) tuple (CPython os.path
# semantics). No filesystem access, so the output is deterministic. Brackets
# mark the empty-string results.
import os


def main():
    auto h, t = os.path_split("/a/b/c.txt")
    print(h)  # /a/b
    print(t)  # c.txt
    auto h2, t2 = os.path_split("c.txt")
    print("[" + h2 + "]")  # []
    print(t2)  # c.txt
    auto h3, t3 = os.path_split("/a/b/")
    print(h3)  # /a/b
    print("[" + t3 + "]")  # []
    auto h4, t4 = os.path_split("/")
    print(h4)  # /
    print("[" + t4 + "]")  # []

    auto r, e = os.path_splitext("/a/b.txt")
    print(r)  # /a/b
    print(e)  # .txt
    auto r2, e2 = os.path_splitext("/a/.bashrc")
    print(r2)  # /a/.bashrc
    print("[" + e2 + "]")  # []
    auto r3, e3 = os.path_splitext("a.b.c")
    print(r3)  # a.b
    print(e3)  # .c
    auto r4, e4 = os.path_splitext("a.")
    print(r4)  # a
    print(e4)  # .
