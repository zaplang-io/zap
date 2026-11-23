# Golden fixture (#1311, decisions/lang/1311-filesystem-notfound-error.md): a
# filesystem read distinguishes a missing path from other failures by exception
# TYPE, while every failure stays catchable by `except Exception`.
#
#   - a missing path -> FileNotFoundError (catchable as FileNotFoundError, as its
#     parent OSError, and as Exception);
#   - a directory (a non-missing failure) -> OSError, NOT FileNotFoundError
#     (open fails EACCES on Windows / read fails EISDIR on POSIX — a non-ENOENT
#     errno either way, so the type is OSError on every platform), still caught
#     by a bare `except Exception`;
#   - a successful read returns the contents;
#   - read_bytes classifies ENOENT the same way.
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

    # (3) A directory is a non-missing failure: OSError, NOT FileNotFoundError.
    # The inner FileNotFoundError clause must NOT fire; the enclosing OSError
    # clause catches it.
    try:
        try:
            print(fs.read_file(d))
        except FileNotFoundError:
            print("WRONG:dir-as-filenotfound")
    except OSError:
        print("dir:oserror-not-filenotfound")

    # ... and the same directory read is still caught by a bare `except Exception`.
    try:
        print(fs.read_file(d))
    except Exception:
        print("dir:exception")

    # (4) read_bytes classifies ENOENT as FileNotFoundError too.
    try:
        auto _b = fs.read_bytes(missing)
    except FileNotFoundError:
        print("missing-bytes:filenotfound")

    fs.rmtree(d)
