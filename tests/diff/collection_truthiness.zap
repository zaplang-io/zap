# diff: match
# Container truthiness (#382): a list or dict used directly in a boolean
# context is falsy when empty and truthy otherwise (Python / D-0022). Fixed by
# routing container `truthy`/`bool()` lowering through the size runtime; Zap now
# agrees with CPython (empty-falsy / full-truthy / dict-empty-falsy).
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
