# Golden fixture (#1582, decisions/lang/1582-regex-error.md): the user-visible
# exception hierarchy for the dedicated regex error type, pinned with user
# `raise`s so it holds independently of any PCRE2 behaviour:
# RegexError ⊂ ValueError ⊂ Exception, and the distinctions hold in both
# directions (a parent ValueError is NOT caught by a child RegexError clause,
# and neither is caught by an unrelated clause). `except E as e` binds the
# message string.
def main():
    # RegexError is caught by name ...
    try:
        raise RegexError("bad-pattern")
    except RegexError as e:
        print(e)  # bad-pattern

    # ... by its parent ValueError (so existing handlers keep working) ...
    try:
        raise RegexError("as-valueerror")
    except ValueError as e:
        print(e)  # as-valueerror

    # ... and by Exception.
    try:
        raise RegexError("as-exception")
    except Exception as e:
        print(e)  # as-exception

    # A plain ValueError is NOT caught by the child `except RegexError`; it
    # propagates to the enclosing `except ValueError`.
    try:
        try:
            raise ValueError("plain-valueerror")
        except RegexError:
            print("WRONG:regexerror-caught-valueerror")
    except ValueError as e:
        print(e)  # plain-valueerror

    # Neither type is caught by an unrelated clause; the RegexError clause that
    # follows it wins.
    try:
        raise RegexError("not-typeerror")
    except TypeError:
        print("WRONG:typeerror-caught")
    except RegexError as e:
        print(e)  # not-typeerror
