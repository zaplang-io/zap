# Borrowed-bytes dict probes (#1082): `d.get(v, default)` and `v in d` look up a
# str-keyed dict with a non-owning view instead of an owned key — byte-identical
# result to the owned-key path, with no transient key allocation per probe. Only
# lookups and counts are printed, so the golden is independent of hash iteration
# order (#1066).
def main():
    dict[str, int64] d = {}
    d["alpha"] = 1
    d["beta"] = 2
    d["gamma"] = 3
    str probe = "alpha!!"
    strview v = probe.view(0, 5)  # "alpha"
    print(v in d)  # True
    print(d.get(v, -1))  # 1
    strview miss = probe.view(0, 4)  # "alph" — absent
    print(miss in d)  # False
    print(d.get(miss, -1))  # -1

    # The headline: a lookup loop driven by a borrow, materializing no transient
    # key per iteration (the dict_string motivation, slice variant).
    int64 total = 0
    for i in range(0, 1000):
        strview key = probe.view(0, 5)
        if key in d:
            total = total + d.get(key, 0)
    print(total)  # 1000
