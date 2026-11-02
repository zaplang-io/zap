# #583: a shadow root containing an alloca's stack address cannot relocate the
# GC reference stored inside that alloca. Force sparse-heap collections and
# reload locals/parameters across calls, assignments, scope joins and finally.
# The same program must work under the non-moving Boehm collector.
struct Holder:
    str label
    list[bytes] values

def make_holder(int64 generation) -> Holder:
    auto holder = Holder("kept-" + str(generation), [bytes(str(generation))])
    for i in range(0, 3000):
        auto garbage = [i, i + 1, i + 2]
    return holder

def churn():
    for i in range(0, 5000):
        auto garbage = [str(i), str(i + 1)]
    gc.collect()

def verify(Holder holder, int64 generation):
    # This parameter is also spilled to a reference local by ZIR.
    gc.collect()
    if holder.label != "kept-" + str(generation):
        raise ValueError("relocated holder label changed")
    if len(holder.values) != 1 or bytes(holder.values[0]) != bytes(str(generation)):
        raise ValueError("relocated holder list changed")

def main():
    auto holder = make_holder(0)
    auto text = "local-" + str(7)
    auto payload = bytes([0, 255, 13, 10])
    list[int64] numbers = [11, 22]
    for generation in range(0, 4):
        holder = make_holder(generation)
        try:
            churn()
            verify(holder, generation)
            scope:
                spawn churn()
            verify(holder, generation)
            if text != "local-7" or payload != bytes([0, 255, 13, 10]):
                raise ValueError("reference local changed")
            numbers.append(generation)
            raise ValueError("expected cleanup path")
        except ValueError as message:
            if message != "expected cleanup path":
                raise ValueError("local root regression: " + message)
            verify(holder, generation)
        finally:
            verify(holder, generation)
    if len(numbers) != 6 or numbers[0] != 11 or numbers[5] != 3:
        raise ValueError("mutable list local changed")
    print("relocated reference locals survive calls, scopes, and cleanup")
