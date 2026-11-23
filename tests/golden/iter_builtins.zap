# Golden fixture: idiomatic-loop built-ins any / all / reversed (#1285).
#
# `any`/`all` reduce a list[bool] to a bool (any([]) is False, all([]) is True).
# `reversed(xs)` returns a NEW list with the elements reversed, leaving `xs`
# unchanged, so it drops straight into a `for` loop.

def main():
    auto flags = [True, False, True]
    print(any(flags))  # True
    print(all(flags))  # False
    print(any([False, False]))  # False
    print(all([True, True]))  # True
    print(any([]))  # False
    print(all([]))  # True

    auto nums = [1, 2, 3, 4]
    list[int64] r = reversed(nums)
    for x in r:
        print(x)  # 4, 3, 2, 1

    # The original list is unchanged.
    for y in nums:
        print(y)  # 1, 2, 3, 4

    # reversed drops into a for loop directly, over any element type.
    for s in reversed(["a", "b", "c"]):
        print(s)  # c, b, a
