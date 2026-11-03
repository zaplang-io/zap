# Golden fixture: `**kwargs` keyword-collector parameter (#601). A trailing
# `V **name` parameter collects any extra keyword arguments into a homogeneous
# `dict[str, V]` (V is the declared value type), mirroring how a `TYPE... name`
# variadic collects extra positionals into a `list`. The body treats it as an
# ordinary dict: `len`, `.keys()`, and indexing all work. Summing the collected
# values is order-independent, so the golden does not depend on dict iteration
# order.
def tally(int64 base, int64 **counts) -> int64:
    auto total = base
    for k in counts.keys():
        total = total + counts[k]
    return total


def ntags(str prefix, str **tags) -> int64:
    print(prefix)
    return len(tags)


def main():
    print(tally(0))
    print(tally(100))
    print(tally(0, a=1, b=2, c=3))
    print(tally(10, x=5, y=5))
    print(ntags("p"))
    print(ntags("q", one="1", two="2"))
