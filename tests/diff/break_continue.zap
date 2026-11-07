# diff: match
# break and continue match CPython (epic #374, walled garden). `break` leaves the
# innermost loop immediately; `continue` skips to the next iteration. Tested in a
# single loop and across nested loops (inner break does not leave the outer
# loop). Sums pin which iterations ran.
def main():
    auto t = 0
    for i in range(5):
        if i == 3:
            break
        t = t + i
    print(t)
    auto u = 0
    for i in range(6):
        if i % 2 == 0:
            continue
        u = u + i
    print(u)
    auto pairs = 0
    for i in range(3):
        for j in range(3):
            if j == 2:
                break
            pairs = pairs + 1
    print(pairs)
