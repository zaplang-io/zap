# Compile-fail fixture: a nested collection has no stable hash, so it cannot be
# a set element (#625). The typed empty constructor now parses a composite
# element type (#703), so `set[list[int64]]()` reaches the same unhashable guard
# as the literal / annotation forms and is rejected with a stable coded
# diagnostic (ZAP6001) — never an internal compiler error, and no longer the
# misleading "expects an element type name".
def main():
    auto s = set[list[int64]]()
    print(len(s))
