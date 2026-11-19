# Golden fixture: fs tempfile (ticket #979, D-0239). Exercises mkstemp (unique
# names, O_EXCL create, prefix/suffix honored, a usable read+write fd),
# gettempdir, and write_atomic (atomic overwrite + a same-dir temp that leaves
# no half-written leftover), plus the catchable error paths. Prints only
# deterministic results (bools, lengths, contents) — never the random temp
# path. Cleans up after itself.
import os
import fs
import io


def main():
    # gettempdir: a non-empty platform temp directory.
    auto tmp = fs.gettempdir()
    print(len(tmp) > 0)  # True

    # mkstemp: two calls never collide, and both files exist on disk.
    auto fd1, p1 = fs.mkstemp()
    auto fd2, p2 = fs.mkstemp()
    print(p1 != p2)  # True
    print(fs.exists(p1))  # True
    print(fs.exists(p2))  # True

    # The returned fd is a usable, writable fd: write through it, close, read
    # the bytes back from the path.
    auto n = io.write(fd1, "hello")
    print(n)  # 5
    io.close_fd(fd1)
    io.close_fd(fd2)
    print(fs.read_file(p1))  # hello
    os.remove(p1)
    os.remove(p2)

    # prefix / suffix are honored (checked on the basename, not the random run).
    auto fd3, p3 = fs.mkstemp("build-", ".log")
    auto base = os.path_basename(p3)
    print(base.startswith("build-"))  # True
    print(base.endswith(".log"))  # True
    io.close_fd(fd3)
    os.remove(p3)

    # An explicit dir places the file there.
    auto d = fs.mkdtemp()
    auto fd4, p4 = fs.mkstemp("x", "", d)
    print(os.path_dirname(p4) == d)  # True
    io.close_fd(fd4)
    fs.rmtree(d)

    # write_atomic: create then overwrite in place; the content is always the
    # complete new file, never a truncated one.
    auto wd = fs.mkdtemp()
    auto target = os.path_join(wd, "out.txt")
    fs.write_atomic(target, "v1")
    print(fs.read_file(target))  # v1
    fs.write_atomic(target, "second-write-longer")
    print(fs.read_file(target))  # second-write-longer

    # The same-dir temp was renamed away, so the directory holds exactly the
    # target — no leftover ".zaptmp" sibling.
    auto entries = fs.listdir(wd)
    print(len(entries))  # 1
    print(entries[0] == "out.txt")  # True
    fs.rmtree(wd)

    # Error paths are catchable (the lowering uses a throwing call): mkstemp and
    # write_atomic into a non-existent directory both raise.
    try:
        auto bfd, bp = fs.mkstemp("x", "", "/no/such/zap/dir/zzz979")
        print("not reached")
    except Exception as e:
        print("mkstemp caught")  # mkstemp caught

    try:
        fs.write_atomic("/no/such/zap/dir/zzz979/out.txt", "data")
        print("not reached")
    except Exception as e:
        print("write_atomic caught")  # write_atomic caught
