# Golden fixture: fs.glob / fs.fnmatch (ticket #1033, D-0216). Builds a small
# tree under a fresh mkdtemp sandbox and exercises pure-string fnmatch plus
# single-level, recursive (**), and segment globs. glob returns paths RELATIVE
# to the root, sorted, so the output is deterministic. Cleans up with rmtree.
import os
import fs


def main():
    auto d = fs.mkdtemp()
    fs.write_file(os.path_join(d, "a.txt"), "")
    fs.write_file(os.path_join(d, "b.txt"), "")
    fs.write_file(os.path_join(d, "c.log"), "")
    fs.makedirs(os.path_join(d, "sub/deep"))
    fs.write_file(os.path_join(d, "sub/d.txt"), "")
    fs.write_file(os.path_join(d, "sub/e.log"), "")
    fs.write_file(os.path_join(d, "sub/deep/f.txt"), "")

    # fnmatch: whole-string shell-glob match.
    print(fs.fnmatch("a.txt", "*.txt"))  # True
    print(fs.fnmatch("a.log", "*.txt"))  # False
    print(fs.fnmatch("a.txt", "a.???"))  # True
    print(fs.fnmatch("ab", "[ab][ab]"))  # True
    print(fs.fnmatch("ac", "[!x]c"))  # True

    # glob: single-level *.txt (a.txt, b.txt).
    auto g1 = fs.glob(d, "*.txt")
    print(len(g1))  # 2
    for i in range(0, len(g1)):
        print(g1[i])

    # glob: recursive **/*.txt across directory levels.
    auto g2 = fs.glob(d, "**/*.txt")
    print(len(g2))  # 4
    for i in range(0, len(g2)):
        print(g2[i])

    # glob: an explicit segment (sub/*.log).
    auto g3 = fs.glob(d, "sub/*.log")
    print(len(g3))  # 1
    print(g3[0])

    # no match -> empty list.
    print(len(fs.glob(d, "*.md")))  # 0

    fs.rmtree(d)
