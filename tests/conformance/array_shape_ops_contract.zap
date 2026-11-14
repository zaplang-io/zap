# claim: ARR-11
# SPEC §15 (#511): shape ops. reshape and transpose/`.T` are views (share the
# parent buffer); flatten is a copy; concatenate/stack are fresh copies with an
# optional axis=. A size/shape/axis fault raises a catchable ValueError.
def main():
    auto a = array[i64]([1, 2, 3, 4, 5, 6])
    auto m = a.reshape(2, 3)
    print("reshape", m.ndim, sum(m))
    print("T", sum(m.T), m.T.shape[0])
    print("flat", m.flatten().ndim, sum(m.flatten()))
    auto p = array[i64]([[1, 2], [3, 4]])
    auto q = array[i64]([[5, 6], [7, 8]])
    print("concat", sum(concatenate([p, q])), concatenate([p, q], axis=1).shape[1])
    print("stack", stack([p, q]).ndim, sum(stack([p, q])))
    try:
        print(sum(a.reshape(4, 2)))
    except ValueError:
        print("err")
