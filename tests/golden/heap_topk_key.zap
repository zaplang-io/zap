# Golden fixture: heap.nlargest / heap.nsmallest WITH a key= selector (#978,
# D-0236). The key is a first-class fn(T) -> K; elements are ordered by their
# key, and the ELEMENTS (not the keys) are returned, in priority order. Covers
# a key that inverts the natural order, a key over structs (order by a field),
# and STABILITY: among equal keys the element that appeared earlier in the input
# comes first (Python heapq semantics).
struct Person:
    str name
    int64 age

def by_age(Person p) -> int64:
    return p.age

def neg(int64 x) -> int64:
    return -x

def main():
    auto xs = [5, 1, 3, 2, 4]
    # key=neg inverts the ordering, so "largest by -x" are the smallest x, and
    # "smallest by -x" are the largest x.
    print(heap.nlargest(2, xs, key=neg))  # [1, 2]
    print(heap.nsmallest(2, xs, key=neg))  # [5, 4]

    # Order Person values by age; the whole Person flows through (shown by name).
    auto people = [
        Person("ada", 36),
        Person("bo", 20),
        Person("cy", 48),
        Person("di", 20),
    ]
    auto oldest = heap.nlargest(2, people, key=by_age)
    print(oldest[0].name)  # cy
    print(oldest[1].name)  # ada

    # Ties on age 20: bo (index 1) appeared before di (index 3), so the stable
    # order keeps bo first.
    auto youngest = heap.nsmallest(2, people, key=by_age)
    print(youngest[0].name)  # bo
    print(youngest[1].name)  # di

    # key= with n > len returns every element, fully ordered by key.
    auto all_by_age = heap.nsmallest(99, people, key=by_age)
    print(len(all_by_age))  # 4
    print(all_by_age[0].name)  # bo  (youngest, stable)
    print(all_by_age[3].name)  # cy  (oldest)
