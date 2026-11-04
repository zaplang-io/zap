# Golden fixture: bare generator expressions `(x for x in xs)` (#596, epic #614
# D4). A genexpr is LAZY — it yields on demand through the I#67 generator
# machinery and builds no intermediate list.
#
# (1) Laziness proof: the generator `produced(n)` prints a marker `200 + i` as
#     each item is produced. A genexpr over it, consumed by a `for` loop that
#     breaks after 2 items, must produce ONLY those 2 — so markers 201 and 202
#     print but 203, 204, 205 never do. The interleaving (marker, then the
#     doubled value) shows items are pulled one at a time, not up front.
# (2) `sum(x*x for x in range(5))` as the sole call argument sums 0+1+4+9+16=30
#     with no intermediate list.
# (3) A genexpr bound to a name is a standalone Iterator consumed once: filtering
#     xs for y > 2 gives 3 and 4, summing to 7.
def produced(int64 n):
    auto i = 0
    while i < n:
        i = i + 1
        print(200 + i)
        yield i

def main():
    # (1) lazy consumption with an early break over a side-effecting generator
    auto count = 0
    for v in (x * 2 for x in produced(5)):
        print(v)
        count = count + 1
        if count == 2:
            break

    # (2) sum over a genexpr — no intermediate list
    print(sum((x * x for x in range(5))))

    # (3) a genexpr bound to a name, then consumed lazily by a for-loop
    list[int64] xs = [1, 2, 3, 4]
    auto g = (y for y in xs if y > 2)
    auto s = 0
    for z in g:
        s = s + z
    print(s)
