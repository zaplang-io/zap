# Golden fixture: bytearray catchable faults (#1385).
#
# An out-of-range index raises a catchable IndexError (get and set); a value
# outside 0..=255 raises a catchable ValueError (set, append, extend(list)); a
# negative constructor count and a zero slice step each raise ValueError. After a
# caught fault the bytearray is intact.
def main():
    auto a = bytearray(b"abc")

    # index out of range (get) -> IndexError
    try:
        int64 x = a[5]
        print(x)
    except IndexError:
        print("get IndexError")

    # index out of range (set) -> IndexError
    try:
        a[5] = 1
    except IndexError:
        print("set IndexError")

    # value out of range (set) -> ValueError
    try:
        a[0] = 256
    except ValueError:
        print("set ValueError")

    # value out of range (append) -> ValueError
    try:
        a.append(-1)
    except ValueError:
        print("append ValueError")

    # value out of range in extend(list[int]) -> ValueError
    try:
        a.extend([300])
    except ValueError:
        print("extend ValueError")

    # negative count in the constructor -> ValueError
    try:
        auto bad = bytearray(-1)
        print(len(bad))
    except ValueError:
        print("ctor ValueError")

    # a zero slice step -> ValueError
    try:
        auto sl = a[::0]
        print(len(sl))
    except ValueError:
        print("slice ValueError")

    # the bytearray is intact after the caught faults.
    print(str(bytes(a)))  # abc
    print(len(a))  # 3
