# Golden fixture (#1582): an *uncaught* regex failure still reports the normal
# Zap exception diagnostic (the PCRE2 message carried by `RegexError`) and exits
# 1 — it is not silently swallowed now that it has a dedicated type. A bare
# `except ValueError` here would still catch it (RegexError ⊂ ValueError); with
# no handler at all it unwinds to the top-level handler (`zap_uncaught`), which
# reports "regex error: could not compile pattern: ..." and exits 1. Buffered
# stdout is flushed on exit.
def main() -> int64:
    print("reached")
    str s = "x"
    auto m = s =~ m/(/
    print("WRONG:no-raise")
    return 0
