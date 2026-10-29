# claim: TRUTH-3
# SPEC §5 (#382, D-0022): a list or dict used directly in a boolean context is
# falsy when empty and truthy otherwise, matching Python. A list holding a falsy
# element (`[0]`) is still truthy — truthiness is emptiness, not contents.
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
    auto d = {"a": 1}
    if d:
        print("dict-truthy")
