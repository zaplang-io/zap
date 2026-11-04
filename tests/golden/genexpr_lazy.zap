# Golden fixture: bare generator expressions `(x for x in xs)` (#596, epic #614
# D4). A genexpr is LAZY — it yields on demand through the I#67 generator
# machinery and builds no intermediate list.
#
# (1) Laziness proof: `tap(x)` prints a marker `200 + x` as each element is
#     produced, then returns it. A genexpr over `range(5)`, consumed by a `for`
#     loop that breaks after 2 elements, runs `tap` for ONLY those 2 — so markers
#     200 and 201 print but 202, 203, 204 never do. The interleaving (marker,
#     then the doubled value) shows elements are pulled one at a time, not up
#     front. (The genexpr drives a plain `range`, not another generator — a
#     genexpr nested inside a generator is the "nested generators" case I#67
#     deferred.)
# (2) `sum(x*x for x in range(5))` as the sole call argument sums 0+1+4+9+16=30
#     with no intermediate list.
# (3) A genexpr bound to a name is a standalone Iterator consumed once: filtering
#     xs for y > 2 gives 3 and 4, summing to 7.
def tap(int64 x) -> int64:
    print(200 + x)
    return x

def main():
    # (1) lazy consumption with an early break over a side-effecting element
    auto count = 0
    for v in (tap(x) * 2 for x in range(5)):
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
