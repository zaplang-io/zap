# diff: match
# A variadic `TYPE... name` parameter is received as a `list` (SPEC §7, D-0038):
# the trailing arguments are packed into it and the callee iterates / `len`s it.
# Python's `*args` has the same semantics, so stdout must agree. Reduced to
# scalar prints so the comparison does not depend on list repr. Closes the #397
# (conformance FN-4) gap.
def sum_all(int64... nums) -> int64:
    auto total = 0
    for n in nums:
        total = total + n
    return total


def scaled_sum(int64 factor, int64... nums) -> int64:
    auto total = 0
    for n in nums:
        total = total + n
    return factor * total


def count(int64... nums) -> int64:
    return len(nums)


def main():
    print(sum_all())
    print(sum_all(1, 2, 3))
    print(scaled_sum(3, 1, 2, 3))
    print(count())
    print(count(5, 6, 7, 8))
