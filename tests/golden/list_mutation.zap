# Golden fixture: list mutation methods (ticket #977, D-0220). extend / insert /
# remove / count / clear, over int and str lists so the by-value equality that
# count/remove use is exercised for both a scalar and a by-contents element kind.
# A remove() of an absent value raises a catchable ValueError.
def main():
    list[int64] xs = [1, 2, 3]
    xs.append(4)
    xs.extend([5, 6])
    xs.insert(0, 0)  # [0, 1, 2, 3, 4, 5, 6]
    print(len(xs))  # 7
    print(xs[0])  # 0
    print(xs[6])  # 6
    xs.insert(2, 99)  # [0, 1, 99, 2, 3, 4, 5, 6]
    print(xs[2])  # 99
    print(xs.count(99))  # 1
    xs.remove(99)  # [0, 1, 2, 3, 4, 5, 6]
    print(len(xs))  # 7
    print(xs[2])  # 2

    list[int64] ys = [1, 2, 2, 3, 2]
    print(ys.count(2))  # 3
    ys.remove(2)  # removes the first 2 -> [1, 2, 3, 2]
    print(ys.count(2))  # 2
    print(len(ys))  # 4

    list[str] ws = ["a", "b", "a", "c"]
    print(ws.count("a"))  # 2 (by contents)
    ws.remove("a")  # [b, a, c]
    print(ws[0])  # b
    print(ws.count("a"))  # 1

    try:
        ws.remove("z")
    except ValueError:
        print("not-found-caught")

    ws.clear()
    print(len(ws))  # 0
