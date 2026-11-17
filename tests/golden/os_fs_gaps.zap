# Golden fixture: os/fs filesystem gaps (ticket #968, D-0211). Exercises the
# mutation + metadata + binary-I/O surface against a fresh mkdtemp sandbox
# (printing only deterministic results — sizes, contents, bools — never the
# non-deterministic temp path or mtime value), then the pure path helpers and
# environ. Cleans up after itself with rmtree.
import os
import fs


def main():
    auto d = fs.mkdtemp()

    # makedirs: recursive directory creation.
    auto sub = os.path_join(d, "a/b/c")
    fs.makedirs(sub)
    print(fs.is_dir(sub))  # True

    # binary read/write round-trip + whole-file read.
    auto f1 = os.path_join(d, "a/hello.txt")
    fs.write_file(f1, "hello")
    auto b = fs.read_bytes(f1)
    print(len(b))  # 5
    fs.write_bytes(os.path_join(d, "a/copy.bin"), b)
    print(fs.read_file(os.path_join(d, "a/copy.bin")))  # hello

    # append.
    fs.append(f1, " world")
    print(fs.read_file(f1))  # hello world

    # stat metadata (size is deterministic; mtime only checked for positivity).
    auto st = fs.stat(f1)
    print(st.size)  # 11
    print(st.is_file)  # True
    print(st.is_dir)  # False
    print(fs.mtime(f1) > 0)  # True

    # rename.
    auto f2 = os.path_join(d, "a/renamed.txt")
    fs.rename(f1, f2)
    print(fs.exists(f1))  # False
    print(fs.exists(f2))  # True

    # copy.
    auto f3 = os.path_join(d, "a/copied.txt")
    fs.copy(f2, f3)
    print(fs.read_file(f3))  # hello world

    # move.
    auto f4 = os.path_join(d, "a/moved.txt")
    fs.move(f3, f4)
    print(fs.exists(f3))  # False
    print(fs.exists(f4))  # True

    # rmtree: recursive remove.
    fs.rmtree(os.path_join(d, "a/b"))
    print(fs.exists(os.path_join(d, "a/b")))  # False
    fs.rmtree(d)
    print(fs.exists(d))  # False

    # path helpers (pure / lexical — fully deterministic).
    print(os.path_normpath("a/b/../c/./d"))  # a/c/d
    print(os.path_normpath("/x/../y"))  # /y
    print(os.path_normpath("a/../.."))  # ..
    print(os.path_normpath("./a"))  # a
    print(os.path_isabs("/x"))  # True
    print(os.path_isabs("x"))  # False
    print(os.sep())  # /
    print(os.path_relpath("/a/b/c", "/a/d"))  # ../b/c
    print(os.path_relpath("/a/b", "/a/b"))  # .
    print(os.path_relpath("/a/b/c/d", "/a/b"))  # c/d
    print(os.path_abspath("/a/b/../c"))  # /a/c
    print(os.path_isabs(os.path_realpath(".")))  # True
    print(os.path_expanduser("/no/tilde"))  # /no/tilde

    # environ: a variable set in-process is enumerated.
    os.setenv("ZAP_FS_TEST_968", "yes")
    auto env = os.environ()
    print(env["ZAP_FS_TEST_968"])  # yes
