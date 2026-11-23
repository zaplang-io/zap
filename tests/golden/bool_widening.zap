# Golden fixture: bool widens to a wider integer by ZERO-extension (#1344). A
# `bool` is an unsigned 0/1, so `True` widens to +1, not -1. Before the fix, the
# coerce path sign-extended the i1 `True` to -1, which inverted every `bool`
# ordering comparison (`False < True` reported False). Equality was unaffected
# (both operands extend the same way, so distinct values stay distinct), so only
# the magnitude-dependent ordering is exercised here. (Explicit `bool` -> int /
# float casts are rejected by sema today — ZAP4024 — a separate gap, so they are
# not part of this fixture.)

def main():
    # scalar bool ordering (Python: False < True)
    print(str(False < True))  # True
    print(str(True < False))  # False
    print(str(False <= False))  # True
    print(str(True <= True))  # True
    print(str(True > False))  # True
    print(str(False > True))  # False
    print(str(False >= True))  # False
    print(str(True >= False))  # True
    print(str(False < False))  # False
    print(str(True > True))  # False
    print("done")
