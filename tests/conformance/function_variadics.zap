# claim: FN-4
# SPEC §7 (D-0038): a variadic parameter `TYPE... name` is homogeneous and
# received as a `list`, so it iterates and has a `len`. It accepts zero or more
# trailing arguments.
def sum_all(int64... nums) -> int64:
    auto total = 0
    for n in nums:
        total = total + n
    return total


def main():
    print(sum_all())
    print(sum_all(1, 2, 3))
    print(sum_all(10, 20))
