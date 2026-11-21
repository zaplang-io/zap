# Golden fixture: url.unquote / url.unquote_plus `errors` mode (#1242).
#
# The percent decoder interprets the decoded octets as UTF-8. `errors="replace"`
# substitutes U+FFFD for each maximal invalid subpart (matching CPython's
# `urllib.parse.unquote(errors="replace")` / `bytes.decode("utf-8","replace")`
# replacement counting), so leniently decoded W3C-baggage values never raise.
# The default (and `errors="strict"`) validates the decoded octets as UTF-8 and
# raises a catchable ValueError on any invalid sequence, exactly like
# `str(bytes)`; an unrecognized `errors` name raises a catchable LookupError.
import url


def main():
    # --- replace mode: one U+FFFD per maximal invalid subpart ---
    # An isolated invalid lead byte (%FF) is a 1-byte subpart -> a single U+FFFD.
    assert url.unquote("A%FFB", errors="replace") == "A�B"
    assert len(url.unquote("A%FFB", errors="replace")) == 3
    assert url.unquote("%FE", errors="replace") == "�"
    print("replace-ok")

    # --- valid multibyte UTF-8 passes through unchanged under BOTH modes ---
    assert url.unquote("%C3%A9") == "é"  # é   (2-byte), strict default
    assert url.unquote("%C3%A9", errors="replace") == "é"
    assert url.unquote("%E2%82%AC") == "€"  # €   (3-byte)
    assert url.unquote("%E2%82%AC", errors="replace") == "€"
    assert url.unquote("%F0%9F%98%80") == "\U0001F600"  # 😀  (4-byte)
    assert url.unquote("%F0%9F%98%80", errors="replace") == "\U0001F600"
    print("valid-ok")

    # --- truncated / overlong / surrogate -> correct U+FFFD counts ---
    assert len(url.unquote("%C3", errors="replace")) == 1  # truncated 2-byte
    assert len(url.unquote("%E2%82", errors="replace")) == 1  # truncated 3-byte
    assert len(url.unquote("%F0%9F%98", errors="replace")) == 1  # truncated 4-byte
    assert len(url.unquote("%C0%80", errors="replace")) == 2  # overlong 2-byte
    assert len(url.unquote("%E0%80%80", errors="replace")) == 3  # overlong 3-byte
    assert len(url.unquote("%ED%A0%80", errors="replace")) == 3  # surrogate U+D800
    print("edge-ok")

    # --- mixed valid + invalid text, and the `_plus` (+ -> space) variant ---
    assert url.unquote("x%FF%E2%82%ACy", errors="replace") == "x�€y"
    assert url.unquote_plus("a+b%FF", errors="replace") == "a b�"
    print("mixed-ok")

    # --- strict (the default) raises a catchable ValueError on invalid UTF-8 ---
    try:
        print(url.unquote("A%FFB"))
        print("strict-NO-RAISE")
    except ValueError:
        print("strict-raised")

    # --- `errors="strict"` named explicitly raises the same way ---
    try:
        print(url.unquote("%FF", errors="strict"))
        print("strict2-NO-RAISE")
    except ValueError:
        print("strict2-raised")

    # --- strict passes valid multibyte through unchanged (no raise) ---
    assert url.unquote("%E2%82%AC", errors="strict") == "€"
    print("strict-valid-ok")

    # --- an unrecognized `errors` name raises a catchable LookupError ---
    try:
        print(url.unquote("A", errors="ignore"))
        print("mode-NO-RAISE")
    except LookupError:
        print("mode-raised")
