# Golden fixture: string-keyed dict enumeration and the header-merge pattern
# that motivated ticket #299 (building `zaplang-io/requests`). A per-call
# override is merged into a copy of the defaults so the caller's dict is not
# mutated. Checks are order-independent: value/key length sums and membership.
def main():
    auto base = {"accept": "json", "host": "x"}

    # Merge per-call headers into a copy; `base` must be left unchanged.
    auto hdrs = base.copy()
    hdrs["accept"] = "text"  # override
    hdrs["auth"] = "token"  # add

    print(len(base))  # 2
    print(base["accept"])  # json  (original unchanged)
    print(len(hdrs))  # 3
    print(hdrs["accept"])  # text
    print(hdrs["auth"])  # token

    # `for k in d` over string keys: sum key and value lengths (ASCII, so
    # len == byte count), independent of iteration order.
    auto klen = 0
    auto vlen = 0
    for k in hdrs:
        klen = klen + len(k)
        vlen = vlen + len(hdrs[k])
    print(klen)  # 6+4+4 = 14
    print(vlen)  # 4+1+5 = 10

    # items() over the same dict yields the same sums.
    auto pk = 0
    auto pv = 0
    for k, v in hdrs.items():
        pk = pk + len(k)
        pv = pv + len(v)
    print(pk)  # 14
    print(pv)  # 10

    # Membership against the untouched original.
    if "host" in base:
        print(1)  # 1
    else:
        print(0)
    if "auth" in base:
        print(1)
    else:
        print(0)  # 0
