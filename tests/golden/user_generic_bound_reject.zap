# Compile-fail fixture: a user generic container's capability bound is enforced
# at instantiation (D-0139/I#811). `Widget` has no total order, so using it as
# the `K: Comparable` key of a user `Tree` is a compile error whose message names
# the capability and why — never a symptom-level or internal error.
struct Widget:
    int64 w


value struct Tree[K: Comparable, V]:
    K key
    V val


def main():
    Tree[Widget, int64] t = Tree[Widget, int64](key=Widget(w=1), val=0)
    print(0)
