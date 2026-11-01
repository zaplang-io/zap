# claim: GEN-1
# SPEC §4.7 (I#476): retained collection slots and aggregate float fields.
struct Measurement:
    f64 value

enum Color:
    RED
    BLUE

def main():
    list[int64] numbers = [1, 2, 3]
    print("ints", numbers[1], len(numbers))
    list[bool] flags = [True, False]
    print("bools", len(flags))
    list[str] names = ["one", "two"]
    print("strings", names[0])
    dict[str, int64] lookup = {"one": 1}
    print("dict", lookup["one"])
    set[int64] unique = {1, 2, 1}
    print("set", len(unique), 2 in unique)
    list[Color] colors = [Color.RED, Color.BLUE]
    print("enum", colors[1].name)
    list[Measurement] samples = [Measurement(1.5)]
    print("struct float", samples[0].value)
    auto pair = (2.5, 4)
    print("tuple float", pair[0])
    auto queue = heap[int64]()
    queue.push(3)
    queue.push(1)
    print("heap", queue.pop())
    auto ch = channel[int64](1)
    ch <- 7
    print("channel", <-ch)
