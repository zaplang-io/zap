# Golden fixture: list.pop([i]) plus index / reverse / copy (ticket #1095).
# Exhaustive Python-parity coverage:
#   * pop: last / first / middle / negative index, order + element-type
#     preservation (int / float / str / reference-struct), the explicit-stack
#     (Havlak DFS-frame) regression, and empty / out-of-range -> IndexError;
#   * index: found / first-of-duplicates / start / start+stop / negative start,
#     str-by-contents, and not-found -> ValueError (CPython, not IndexError);
#   * reverse: even / odd / empty, in place;
#   * copy: independent membership from the original, but shared referents for
#     reference elements (shallow copy).
# A reference struct witnesses that pop/copy move the stored pointer word, so a
# mutation through one list's element is seen through the other's shared copy.

struct Cell:
    int64 v

def main():
    # --- Explicit stack (the Havlak DFS-frame regression): LIFO pop, no search.
    list[int64] stack = []
    stack.append(10)
    stack.append(20)
    stack.append(30)
    print(stack.pop())  # 30
    print(stack.pop())  # 20
    print(len(stack))  # 1
    print(stack.pop())  # 10
    print(len(stack))  # 0

    # --- pop: last / first / middle / negative, order preserved throughout.
    list[int64] ys = [10, 20, 30, 40, 50]
    print(ys.pop())  # 50 (last, O(1))  -> [10, 20, 30, 40]
    print(ys.pop(0))  # 10 (first)       -> [20, 30, 40]
    print(ys.pop(1))  # 30 (middle)      -> [20, 40]
    print(ys.pop(-1))  # 40 (negative)   -> [20]
    print(len(ys))  # 1
    print(ys[0])  # 20

    # --- pop element-type preservation: float, str, reference struct.
    list[float] fs = [1.5, 2.5, 3.5]
    print(fs.pop())  # 3.5
    print(fs.pop(0))  # 1.5
    print(len(fs))  # 1

    list[str] ws = ["a", "b", "c"]
    print(ws.pop())  # c
    print(ws.pop(0))  # a
    print(ws[0])  # b

    list[Cell] cs = [Cell(v=7), Cell(v=8), Cell(v=9)]
    auto popped = cs.pop()  # the Cell(v=9) reference
    print(popped.v)  # 9
    auto first = cs.pop(0)  # the Cell(v=7) reference
    print(first.v)  # 7
    print(len(cs))  # 1
    print(cs[0].v)  # 8

    # --- pop faults: empty list and out-of-range (both IndexError, catchable).
    list[int64] e = []
    try:
        auto bad = e.pop()
    except IndexError:
        print("empty-pop-caught")
    list[int64] small = [1, 2, 3]
    try:
        auto bad2 = small.pop(10)
    except IndexError:
        print("oor-pop-caught")
    try:
        auto bad3 = small.pop(-10)
    except IndexError:
        print("oor-neg-pop-caught")
    print(len(small))  # 3 (failed pops left it untouched)

    # --- index: found / first-of-duplicates / start / start+stop / negative.
    list[int64] zs = [5, 6, 7, 6, 5]
    print(zs.index(7))  # 2
    print(zs.index(6))  # 1 (first match)
    print(zs.index(6, 2))  # 3 (search from index 2)
    print(zs.index(5, 1, 5))  # 4 (within [1, 5))
    print(zs.index(5, -1))  # 4 (negative start counts from the end)
    try:
        auto nf = zs.index(99)
    except ValueError:
        print("index-missing-caught")
    try:
        auto nf2 = zs.index(5, 1, 4)  # the 5s are at 0 and 4; [1, 4) has neither
    except ValueError:
        print("index-bounded-missing-caught")

    # index by contents for str elements.
    list[str] ss = ["x", "y", "x"]
    print(ss.index("x"))  # 0
    print(ss.index("x", 1))  # 2

    # --- reverse: even length, odd length, empty; all in place.
    list[int64] rv = [1, 2, 3, 4]
    rv.reverse()  # [4, 3, 2, 1]
    print(rv[0])  # 4
    print(rv[3])  # 1
    list[int64] od = [1, 2, 3]
    od.reverse()  # [3, 2, 1]
    print(od[0])  # 3
    print(od[1])  # 2 (the pivot stays put)
    print(od[2])  # 1
    list[int64] em = []
    em.reverse()
    print(len(em))  # 0

    # --- copy: independent membership from the original.
    list[int64] src = [1, 2, 3]
    auto cp = src.copy()
    cp.append(4)
    print(len(src))  # 3 (appending to the copy does not grow the original)
    print(len(cp))  # 4
    cp.pop(0)  # cp -> [2, 3, 4]
    print(src[0])  # 1 (popping the copy does not shift the original)
    print(cp[0])  # 2

    # copy is shallow: reference elements are SHARED, so a mutation through one
    # list's element is visible through the other's.
    list[Cell] bx = [Cell(v=1), Cell(v=2)]
    auto bc = bx.copy()
    auto node = bx[0]
    node.v = 55  # mutate the shared referent through the original
    print(bc[0].v)  # 55 (same Cell object behind both lists)
    bc.pop()  # removing from the copy leaves the original's length alone
    print(len(bx))  # 2
    print(len(bc))  # 1
