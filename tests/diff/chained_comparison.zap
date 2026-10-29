# diff: diverge BUG: chained comparison is not Python-chained. CPython treats
# `a < b < c` as `(a < b) and (b < c)` with b evaluated once; Zap's parser is
# plain left-associative precedence climbing (crates/syntax/src/parser.rs
# parse_bin, all comparison ops at bp 30), so `a < b < c` lowers to `(a < b) < c`
# — comparing a bool against c. This is an UNINTENDED divergence (a real bug to
# triage, not a locked decision): CPython True/False vs Zap's bool-vs-int result.
# Rendered via str().
def main():
    print(str(1 < 3 < 2))
    print(str(3 > 2 > 1))
    print(str(2 < 1 < 3))
