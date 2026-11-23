# Golden fixture (#1320, decisions/lang/1320-os-fs-typed-errors.md): the typed
# error buckets the os/fs/io surface now raises are mutually distinct and each
# catchable by `except Exception`, pinned with user `raise`s so it holds
# independently of any syscall behaviour. `MemoryError` (allocation failures),
# `OSError` (OS / I/O failures) and `ValueError` (bad caller input). The
# FileNotFoundError ⊂ OSError ⊂ Exception edge is pinned by fs_notfound_hierarchy
# (#1311); here we pin the sibling buckets. `except E as e` binds the message.
def main():
    # MemoryError (the bucket for allocation failures) is catchable by name ...
    try:
        raise MemoryError("alloc")
    except MemoryError as e:
        print(e)  # alloc

    # ... and as Exception.
    try:
        raise MemoryError("alloc-as-exception")
    except Exception as e:
        print(e)  # alloc-as-exception

    # OSError is NOT caught by a ValueError clause; the OSError clause wins.
    try:
        raise OSError("io")
    except ValueError:
        print("WRONG:valueerror-caught-oserror")
    except OSError as e:
        print(e)  # io

    # ValueError is NOT caught by an OSError clause.
    try:
        raise ValueError("bad-input")
    except OSError:
        print("WRONG:oserror-caught-valueerror")
    except ValueError as e:
        print(e)  # bad-input

    # OSError is catchable by Exception too.
    try:
        raise OSError("os-as-exception")
    except Exception as e:
        print(e)  # os-as-exception
