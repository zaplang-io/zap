# Golden fixture: bisect over a non-int64 sorted list (#983).
#
# The re-skinned `bisect` works over any `Comparable` list element: str (ordered
# byte-wise) and float64 (IEEE total order), not just int64. The element kind is
# read off the list's type and selects the runtime comparator.
def main():
    auto words = ["apple", "mango", "pear"]
    print(bisect.bisect_left(words, "mango"))
    print(bisect.bisect_right(words, "mango"))
    print(bisect.bisect_left(words, "kiwi"))
    bisect.insort_left(words, "cherry")
    # words == [apple, cherry, mango, pear]
    print(len(words))
    print(words[1])
    auto nums = [1.5, 2.5, 3.5]
    print(bisect.bisect_left(nums, 2.5))
    print(bisect.bisect_right(nums, 2.5))
    bisect.insort_right(nums, 2.5)
    # nums == [1.5, 2.5, 2.5, 3.5]
    print(len(nums))
    print(bisect.bisect_left(nums, 2.5))
    print(bisect.bisect_right(nums, 2.5))
