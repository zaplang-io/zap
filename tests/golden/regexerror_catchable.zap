# Golden fixture (#1582, decisions/lang/1582-regex-error.md): a *runtime* regex
# failure — an invalid pattern that PCRE2 cannot compile, and a bad substitution
# pattern — now raises a dedicated `RegexError` (replacing #1566's provisional
# bare `ValueError`). Because `RegexError` ⊂ `ValueError` ⊂ `Exception`, the same
# fault is caught by `except RegexError`, by `except ValueError` (no migration
# needed for existing handlers), and by `except Exception`. The substitution
# path (`s///`) fails through the same `zap_regex_fail` and is equally catchable.
def main():
    str s = "x"

    # The dedicated type catches the match-compile failure ...
    try:
        auto m = s =~ m/(/
        print("WRONG:no-raise")
    except RegexError:
        print("caught regex")

    # ... and the pre-existing base `except ValueError` still catches it, so no
    # handler written against #1566's provisional type has to change.
    try:
        auto n = s =~ m/(/
        print("WRONG:no-raise")
    except ValueError:
        print("caught value")

    # ... and `except Exception` catches it too.
    try:
        auto p = s =~ m/(/
        print("WRONG:no-raise")
    except Exception:
        print("caught exception")

    # A substitution with a bad pattern raises `RegexError` through the same
    # failure path.
    try:
        auto q = s =~ s/(/x/
        print("WRONG:no-raise")
    except RegexError:
        print("caught subst")
