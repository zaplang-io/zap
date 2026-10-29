# Golden fixture: `xs.sort()` (stable, in place) and `sorted(xs)` (a new list),
# with optional `key=` / `reverse=` (ticket #336). Stable ordering is what AWS
# canonical header/query ordering and deterministic serialization rely on, so
# the duplicate-key cases assert that equal elements keep their input order.

def slen(str s) -> int64:
    # A key projection: order strings by length. Several inputs share a length,
    # so the relative order among them witnesses stability.
    return len(s)

def lower_key(str s) -> str:
    # A str-returning key: case-insensitive ordering.
    return s.lower()

def join(list[str] xs) -> str:
    auto out = ""
    auto first = True
    for x in xs:
        if first:
            out = out + x
            first = False
        else:
            out = out + "," + x
    return out

def join_ints(list[int64] xs) -> str:
    auto out = ""
    auto first = True
    for x in xs:
        if first:
            out = out + str(x)
            first = False
        else:
            out = out + "," + str(x)
    return out

def main():
    # --- int: in-place sort of an unsorted list -----------------------------
    auto a = [3, 1, 2, 5, 4]
    a.sort()
    print(join_ints(a))  # 1,2,3,4,5

    # already sorted stays sorted; reverse-sorted input sorts ascending.
    auto b = [1, 2, 3]
    b.sort()
    print(join_ints(b))  # 1,2,3
    auto c = [9, 7, 5, 3, 1]
    c.sort()
    print(join_ints(c))  # 1,3,5,7,9

    # reverse flag (descending).
    auto d = [3, 1, 2]
    d.sort(reverse=True)
    print(join_ints(d))  # 3,2,1

    # --- int: sorted() returns a new list, input untouched ------------------
    auto src = [4, 2, 3, 1]
    auto out = sorted(src)
    print(join_ints(out))  # 1,2,3,4
    print(join_ints(src))  # 4,2,3,1  (unchanged)
    print(join_ints(sorted(src, reverse=True)))  # 4,3,2,1

    # --- str: lexicographic ordering ----------------------------------------
    auto names = ["grace", "ada", "alan", "ada"]
    names.sort()
    print(join(names))  # ada,ada,alan,grace
    print(join(sorted(["banana", "apple", "cherry"])))  # apple,banana,cherry

    # --- key= projection returning an int (sort by length) ------------------
    # Stability: equal-length words keep their INPUT order ("bb" before "cc"
    # before "aa"; "a" before "b").
    auto w = ["bb", "a", "cc", "b", "aa"]
    print(join(sorted(w, key=slen)))  # a,b,bb,cc,aa
    # reverse=True flips the key order but is still stable among equals: the
    # length-2 group keeps input order (bb,cc,aa), then the length-1 group (a,b).
    print(join(sorted(w, key=slen, reverse=True)))  # bb,cc,aa,a,b
    print(join(w))  # bb,a,cc,b,aa  (sorted() left the input unchanged)

    # in-place sort with a key.
    auto w2 = ["ccc", "a", "bb"]
    w2.sort(key=slen)
    print(join(w2))  # a,bb,ccc

    # --- key= projection returning a str (case-insensitive) -----------------
    auto mixed = ["Banana", "apple", "Cherry"]
    print(join(sorted(mixed, key=lower_key)))  # apple,Banana,Cherry

    # --- empty and singleton lists ------------------------------------------
    auto empty = sorted([])
    print(len(empty))  # 0
    auto one = [42]
    one.sort()
    print(join_ints(one))  # 42
    print(len(sorted([7])))  # 1
