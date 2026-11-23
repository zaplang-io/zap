# Golden fixture (#1320, decisions/lang/1320-os-fs-typed-errors.md): the os/fs/io
# surface raises typed errors, generalizing #1311 beyond read_file/read_bytes.
#
#   - a missing path raises FileNotFoundError (catchable as FileNotFoundError, as
#     its parent OSError, and as Exception) — shown via fs.size and os.listdir;
#   - a directory where a file is expected raises OSError, NOT FileNotFoundError,
#     and no longer OOM-aborts (the #1320 fstat guard in zap_read_all_fd) — shown
#     via fs.read_file(dir); still caught by a bare except Exception;
#   - a bad io.open mode raises ValueError (caller input), caught by Exception too.
#
# Deterministic on every platform: ENOENT is universal; a directory read is a
# non-ENOENT failure on every platform (POSIX open succeeds then read -> EISDIR,
# Windows open -> EACCES), so the TYPE is OSError everywhere (we assert the type,
# never the errno or message text). Only fixed labels are printed, never the
# non-deterministic temp path.
import os
import fs
import io


def main():
    auto d = fs.mkdtemp()
    auto missing = os.path_join(d, "nope")

    # (1) A missing path: fs.size raises FileNotFoundError, catchable at every
    # level of the hierarchy.
    try:
        auto _s1 = fs.size(missing)
    except FileNotFoundError:
        print("size-missing:filenotfound")
    try:
        auto _s2 = fs.size(missing)
    except OSError:
        print("size-missing:oserror")
    try:
        auto _s3 = fs.size(missing)
    except Exception:
        print("size-missing:exception")

    # (2) os.listdir on a missing directory also raises FileNotFoundError.
    try:
        auto _l = os.listdir(missing)
    except FileNotFoundError:
        print("listdir-missing:filenotfound")

    # (3) A directory where a file is expected: OSError, NOT FileNotFoundError
    # (the inner clause must not fire), and it no longer OOM-aborts.
    try:
        try:
            auto _c1 = fs.read_file(d)
        except FileNotFoundError:
            print("WRONG:dir-filenotfound")
    except OSError:
        print("readfile-dir:oserror")
    # ... still caught by a bare except Exception.
    try:
        auto _c2 = fs.read_file(d)
    except Exception:
        print("readfile-dir:exception")

    # (4) A bad io.open mode is a caller-input error -> ValueError, under Exception.
    try:
        auto _fd = io.open(os.path_join(d, "x"), "zzz")
    except ValueError:
        print("open-badmode:valueerror")

    fs.rmtree(d)
