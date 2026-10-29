# diff: diverge BUG: using a list or dict directly in a boolean context — `if
# xs:` / `if not d:` — crashes Zap codegen (panic "Found PointerValue but
# expected the IntValue variant", exit 101) instead of applying Python
# truthiness (empty is falsy, non-empty truthy). SCALAR/string truthiness works
# (see truthiness.zap); only container truthiness faults. A real codegen bug to
# triage. CPython: empty-falsy / full-truthy / dict-empty-falsy; Zap: crash.
def main():
    list[int64] empty = []
    if not empty:
        print("empty-list-falsy")
    list[int64] full = [0]
    if full:
        print("full-list-truthy")
    dict[str, int64] ed = {}
    if not ed:
        print("empty-dict-falsy")
