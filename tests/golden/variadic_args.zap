# Golden fixture: variadic parameters (#397, FN-4). A `TYPE... name` parameter
# is received as a `list` (SPEC §7, D-0038): the callee iterates it, takes its
# `len`, and indexes it; the caller packs the trailing arguments into that list,
# so a bare call passes an empty one. Also exercises a fixed parameter preceding
# the variadic. Reduced to scalar prints so the golden does not depend on repr.
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


def first(int64... nums) -> int64:
    return nums[0]


def main():
    print(sum_all())
    print(sum_all(1, 2, 3))
    print(sum_all(10, 20))
    print(scaled_sum(3, 1, 2, 3))
    print(count())
    print(count(5, 6, 7, 8))
    print(first(9, 8, 7))
