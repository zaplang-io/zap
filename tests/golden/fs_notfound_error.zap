# Golden fixture (#1311, decisions/lang/1311-filesystem-notfound-error.md): a
# filesystem read on a MISSING path raises the typed `FileNotFoundError`, which
# is catchable at every level of its hierarchy (FileNotFoundError ⊂ OSError ⊂
# Exception) and is NOT caught by an unrelated clause; a successful read returns
# the contents; and `read_bytes` classifies ENOENT the same way.
#
# The missing-path (ENOENT) trigger is deterministic and identical on every
# platform. The complementary "a non-missing failure raises OSError, NOT
# FileNotFoundError" distinction is pinned deterministically by the
# `fs_notfound_hierarchy` golden (via user `raise`s), because the real-syscall
# non-ENOENT failures vary by platform (open succeeds on a directory on POSIX
# but fails EACCES on Windows) and reading a directory hits a separate, unrelated
# read-buffer bug — both tracked as a follow-up, outside #1311's error-typing.
#
# Only deterministic labels and the known file content are printed, never the
# non-deterministic temp path.
import os
import fs


def main():
    auto d = fs.mkdtemp()

    # (1) A successful read returns the contents.
    auto good = os.path_join(d, "good.txt")
    fs.write_file(good, "hello-1311")
    print(fs.read_file(good))  # hello-1311

    auto missing = os.path_join(d, "nope.txt")

    # (2) A missing path raises FileNotFoundError ...
    try:
        print(fs.read_file(missing))
    except FileNotFoundError:
        print("missing:filenotfound")

    # ... is catchable as its parent OSError ...
    try:
        print(fs.read_file(missing))
    except OSError:
        print("missing:oserror")

    # ... and as Exception.
    try:
        print(fs.read_file(missing))
    except Exception:
        print("missing:exception")

    # (3) It is NOT caught by an unrelated clause; the FileNotFoundError clause
    # that follows wins.
    try:
        print(fs.read_file(missing))
    except ValueError:
        print("WRONG:valueerror-caught")
    except FileNotFoundError:
        print("missing:not-valueerror")

    # (4) read_bytes classifies ENOENT as FileNotFoundError too.
    try:
        auto _b = fs.read_bytes(missing)
    except FileNotFoundError:
        print("missing-bytes:filenotfound")

    fs.rmtree(d)
