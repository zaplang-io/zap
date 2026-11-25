# Golden fixture (#1492, Epic #1489,
# decisions/lang/1489-null-reference-error.md): the user-visible hierarchy and
# catch behaviour of the `NullReferenceError` built-in, pinned with user
# `raise`s so it holds independently of the deref-site codegen that will raise it
# automatically (#1493). NullReferenceError sits flat under Exception, so it is
# caught by its own name and by `except Exception`, but NOT by an unrelated
# clause; `except E as e` binds the located message string, which round-trips the
# location (and field/type when known) exactly.
def main():
    # Caught by its own name; the located message round-trips verbatim.
    try:
        raise NullReferenceError(
            "null dereference reading field 'name' of 'Node' at foo.zap:7",
        )
    except NullReferenceError as e:
        print(e)  # null dereference reading field 'name' of 'Node' at foo.zap:7

    # Caught by its Exception parent (flat placement).
    try:
        raise NullReferenceError("null dereference at bar.zap:12")
    except Exception as e:
        print(e)  # null dereference at bar.zap:12

    # NOT caught by an unrelated clause: the ValueError handler is skipped and
    # the following NullReferenceError handler wins.
    try:
        raise NullReferenceError("not-a-valueerror at baz.zap:3")
    except ValueError:
        print("WRONG:valueerror-caught-nullreference")
    except NullReferenceError as e:
        print(e)  # not-a-valueerror at baz.zap:3

    # In the other direction, `except NullReferenceError` does NOT catch an
    # unrelated ValueError; it propagates to the enclosing `except ValueError`.
    try:
        try:
            raise ValueError("plain-valueerror")
        except NullReferenceError:
            print("WRONG:nullreference-caught-valueerror")
    except ValueError as e:
        print(e)  # plain-valueerror

    # A located message with neither field nor owning type still round-trips (the
    # location is the required part of the shape).
    try:
        raise NullReferenceError("null dereference indexing at grid.zap:42")
    except NullReferenceError as e:
        print(e)  # null dereference indexing at grid.zap:42
