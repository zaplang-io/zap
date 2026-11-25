# Golden fixture: list(iterable) + dict(pairs/copy/kwargs) constructors (#1460).
# list() is checked over ordered sources (range/str/list) so element order is
# deterministic; dict() is checked by key lookup and len (never repr), since
# dict iteration order is unspecified (#1066).
def main():
    # --- list(iterable) ---
    print(list(range(3)))  # [0, 1, 2]
    print(list("abc"))  # ['a', 'b', 'c']
    auto base = [10, 20, 30]
    print(list(base))  # [10, 20, 30]
    print(len(list(range(0))))  # 0

    # --- dict(pairs): an iterable of (key, value) 2-tuples ---
    auto d = dict([(1, "a"), (2, "b")])
    print(d[1])  # a
    print(d[2])  # b
    print(len(d))  # 2

    # --- dict(other_dict): a copy ---
    auto c = dict(d)
    print(c[1])  # a
    print(len(c))  # 2

    # --- dict(**kwargs): string-keyed ---
    auto k = dict(x=1, y=2)
    print(k["x"])  # 1
    print(k["y"])  # 2
    print(len(k))  # 2
