# Regression for #1262: a bare payload value supplied for an `Optional[T]`
# position that sema accepts must auto-wrap to a tagged/boxed `Some(payload)`,
# never store the raw scalar/pointer. Reading such a field back (is_some /
# unwrap / unwrap_or) used to SIGSEGV at 0x1 because `.unwrap()` dereferenced the
# integer as an Optional object — a memory-safety violation in a checked
# program. This is the symmetric partner of the bare-`None` lowering
# (#1075/#1080): the same type-contextual chokepoint, now wrapping the non-`None`
# payload half.
#
# Scope note (see the #1262 ADR): sema accepts a bare payload only in these
# positions — constructor fields (positional/keyword/omitted default), field
# assignment, index assignment, `.append`, and dict-value assignment — so those
# are the ones that crash today and the ones exercised here. Bare payloads in
# `return` / `Optional[T]` parameter / typed-binding / list-literal positions are
# still clean sema type errors (a separate implicit-widening design question),
# so they are deliberately absent. All reads below are native
# (is_some/unwrap/unwrap_or) so the tagged-vs-raw storage is actually observed —
# it only matters once the native collector runs.

value struct Delta:
    Optional[int64] day = None
    Optional[int64] hour = None

struct Item:
    int64 value

# A reference struct so field assignment (`n.name = ...`) is unambiguous.
struct Node:
    Optional[str] name = None
    Optional[Item] slot = None

def main():
    # The issue's exact minimal repro.
    Delta d0 = Delta(day=1)
    print(d0.day.unwrap())  # 1

    # Zero payload: a raw 0 is indistinguishable from a null Optional pointer, so
    # the discriminant tag is what keeps is_some() correct.
    Delta d1 = Delta(day=0, hour=9)
    print(d1.day.is_some())  # True
    print(d1.day.unwrap())  # 0
    print(d1.hour.unwrap())  # 9

    # Positional bare payloads.
    Delta d2 = Delta(2, 3)
    print(d2.day.unwrap())  # 2
    print(d2.hour.unwrap())  # 3

    # Omitted field keeps its declared `None` default.
    Delta d3 = Delta(day=7)
    print(d3.day.unwrap())  # 7
    print(d3.hour.is_none())  # True

    # Explicit None / explicit Some controls stay unchanged.
    Delta d4 = Delta(day=None, hour=Optional.Some(5))
    print(d4.day.is_none())  # True
    print(d4.hour.unwrap())  # 5

    # An already-Optional binding is not double-wrapped into Some(Some(..)).
    Optional[int64] existing = Optional.Some(6)
    Delta d5 = Delta(day=existing)
    print(d5.day.unwrap())  # 6

    # Reference payloads: a bare str and a bare struct both wrap (a different
    # physical representation from a scalar).
    Node n = Node(name="hi", slot=Item(42))
    print(n.name.unwrap())  # hi
    print(n.slot.unwrap().value)  # 42

    # Field assignment: obj.field = <bare payload>, scalar and reference.
    Node n2 = Node()
    print(n2.name.is_none())  # True
    n2.name = "set"
    print(n2.name.unwrap())  # set
    n2.slot = Item(7)
    print(n2.slot.unwrap().value)  # 7

    # Collection elements: index assignment, append, and a dict value. (The list
    # is seeded with None/Some — a bare-scalar list *literal* element is a
    # separate sema error, so the bare payload is supplied via assignment.)
    list[Optional[int64]] xs = [None, Optional.Some(3)]
    print(xs[0].is_none())  # True
    print(xs[1].unwrap())  # 3
    xs[0] = 2
    print(xs[0].unwrap())  # 2
    xs.append(6)
    print(xs[2].unwrap())  # 6

    dict[str, Optional[int64]] m = {}
    m["a"] = 10
    print(m["a"].unwrap())  # 10
