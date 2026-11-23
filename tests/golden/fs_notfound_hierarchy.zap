# Golden fixture (#1311, decisions/lang/1311-filesystem-notfound-error.md): the
# user-visible exception hierarchy for the filesystem read-error types, pinned
# with user `raise`s so it holds independently of any syscall behaviour:
# FileNotFoundError ⊂ OSError ⊂ Exception, and the distinctions hold in both
# directions (a parent OSError is NOT caught by a child FileNotFoundError clause,
# and neither is caught by an unrelated clause). `except E as e` binds the
# message string.
def main():
    # FileNotFoundError is caught by name ...
    try:
        raise FileNotFoundError("missing")
    except FileNotFoundError as e:
        print(e)  # missing

    # ... by its parent OSError ...
    try:
        raise FileNotFoundError("as-oserror")
    except OSError as e:
        print(e)  # as-oserror

    # ... and by Exception.
    try:
        raise FileNotFoundError("as-exception")
    except Exception as e:
        print(e)  # as-exception

    # A plain OSError is NOT caught by the child `except FileNotFoundError`; it
    # propagates to the enclosing `except OSError`.
    try:
        try:
            raise OSError("plain-oserror")
        except FileNotFoundError:
            print("WRONG:filenotfound-caught-oserror")
    except OSError as e:
        print(e)  # plain-oserror

    # OSError is itself catchable as Exception.
    try:
        raise OSError("oserror-as-exception")
    except Exception as e:
        print(e)  # oserror-as-exception

    # Neither type is caught by an unrelated clause; the FileNotFoundError clause
    # that follows it wins.
    try:
        raise FileNotFoundError("not-valueerror")
    except ValueError:
        print("WRONG:valueerror-caught")
    except FileNotFoundError as e:
        print(e)  # not-valueerror
