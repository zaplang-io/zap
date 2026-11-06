# Golden fixture: tuples as dict keys (#680, D-0049). A tuple of hashable
# elements is itself hashable (CPython parity): it lowers as a value-struct
# aggregate and routes through the function-pointer key path with a synthesized
# structural `__hash__`/`__eq__` over the element layout. Exercises literal
# construction, get, insert, structural-equality overwrite (a freshly built
# equal tuple hits the same slot), `len`, and `in` — for (int,int), (int,str),
# nested ((int,int),int), (bool,int), and (enum,int) key layouts.
enum Color:
    RED
    GREEN
    BLUE

def main():
    # (int64, int64) keys: get, overwrite by structural equality, len, in.
    auto d = {(1, 2): 10, (3, 4): 20}
    print(len(d))  # 2
    print(d[1, 2])  # 10
    print(d[3, 4])  # 20
    d[1, 2] = 11  # structural key already present -> overwrite
    print(d[1, 2])  # 11
    print(len(d))  # 2  (no new key)
    d[5, 6] = 30  # new key -> insert
    print(len(d))  # 3
    auto k = (1, 2)  # a separately-built equal tuple finds the slot
    print(d[k])  # 11
    if (3, 4) in d:
        print(1)  # 1
    else:
        print(0)
    if (9, 9) in d:
        print(1)
    else:
        print(0)  # 0

    # (int64, str) keys: a str element is compared by value (zap_str_eq).
    auto ds = {(1, "a"): 100, (2, "b"): 200}
    print(len(ds))  # 2
    print(ds[1, "a"])  # 100
    ds[1, "a"] = 101  # structural overwrite
    print(len(ds))  # 2
    print(ds[1, "a"])  # 101
    if (2, "b") in ds:
        print(1)  # 1
    else:
        print(0)
    if (3, "c") in ds:
        print(1)
    else:
        print(0)  # 0

    # Nested tuple keys ((int64,int64), int64): hashed/compared recursively.
    auto dn = {((1, 2), 3): 7, ((1, 2), 4): 8}
    print(len(dn))  # 2
    print(dn[(1, 2), 3])  # 7
    print(dn[(1, 2), 4])  # 8
    if ((1, 2), 3) in dn:
        print(1)  # 1
    else:
        print(0)
    if ((9, 9), 3) in dn:
        print(1)
    else:
        print(0)  # 0

    # (bool, int64) keys.
    auto db = {(True, 1): 5, (False, 1): 6}
    print(db[True, 1])  # 5
    print(db[False, 1])  # 6
    if (True, 1) in db:
        print(1)  # 1
    else:
        print(0)

    # (enum, int64) keys: enum discriminant hashed as an integer word.
    auto dc = {(Color.RED, 1): 50, (Color.BLUE, 1): 60}
    print(dc[Color.RED, 1])  # 50
    print(dc[Color.BLUE, 1])  # 60
    if (Color.GREEN, 1) in dc:
        print(1)
    else:
        print(0)  # 0
