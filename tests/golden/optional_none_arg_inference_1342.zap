# Golden fixture (#1342): a payload-less `Optional.None` passed as a call
# argument or a struct-constructor field infers its type argument `T` from the
# callee's declared parameter / field type, so both Some and None flow through
# those positions and run. Before the fix, `take(Optional.None)` and
# `Box(Optional.None)` failed to lower (ZAP6001 "cannot infer type argument
# 'T'") because arguments were rewritten with no expected type.

struct R:
    int64 value

struct Box:
    Optional[R] slot

def describe(Optional[R] o) -> int64:
    if o.is_some():
        return o.unwrap().value
    return -1

def main():
    # Optional.None as a function argument (the failing position).
    print(describe(Optional.None))  # -1
    # Optional.Some as a function argument (regression).
    print(describe(Optional.Some(R(7))))  # 7
    # Bare None as a function argument (already worked).
    print(describe(None))  # -1

    # Optional.None as a struct-constructor field (the failing position).
    Box empty = Box(Optional.None)
    print(empty.slot.is_some())  # False

    # Optional.Some as a struct-constructor field, then read back through it.
    Box full = Box(Optional.Some(R(42)))
    print(full.slot.is_some())  # True
    print(describe(full.slot))  # 42  (the field read is itself an Optional[R] arg)

    # Bare None as a struct-constructor field (already worked).
    Box bare = Box(None)
    print(bare.slot.is_some())  # False
