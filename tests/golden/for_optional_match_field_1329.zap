# Golden fixture (#1329): matching an `Optional[T]` returned by a method call on
# a `for`-loop receiver must keep the generic enum's instance so both variant
# arms lower and run — including when the receiver is iterated from a `list`-typed
# struct FIELD. Before the fix the monomorphizer declared a `for`-loop variable
# as `Unknown` (unlike a comprehension clause, #1131), so `counter.collect()`
# could not be typed and the `match` failed to lower with ZAP6001 "cannot
# determine the instance of generic enum 'Optional'". The loop variable now binds
# the iterable's element type, mirroring the comprehension path.
#
# This is the zaplang-io/otel#3 shape: a meter iterates its registered
# instruments and collects their optional observations.

struct Point:
    int64 value

struct Counter:
    int64 base

    def collect(self) -> Optional[Point]:
        if self.base < 0:
            return Optional.None
        return Optional.Some(Point(self.base * 10))

# A second instrument whose method returns `Optional[T]` for a DIFFERENT `T`
# (`str`), to wall off the general element-type inference path (not just the
# single `Optional[Point]` instance).
struct Label:
    str name

    def read(self) -> Optional[str]:
        if self.name == "":
            return Optional.None
        return Optional.Some(self.name)

# #1339 (same root cause): `Optional.Some(<loop var>)` nested inside another
# constructor and returned, with the loop iterating a `list` struct FIELD. The
# loop variable's element type must flow into the nested variant call so its
# type argument `T` is inferred. Present and absent cases both exercised.
struct Record:
    int64 value

struct Reply:
    list[Record] records

struct Result:
    Optional[Record] record

def first_record(Reply reply) -> Result:
    for record in reply.records:
        return Result(Optional.Some(record))
    return Result(None)

struct Meter:
    list[Counter] counters
    list[Label] labels

    # The failing form: iterate a `list[Counter]` FIELD, match the method result.
    def sum_points(self) -> int64:
        int64 total = 0
        for counter in self.counters:
            match counter.collect():
                case Optional.Some(point):
                    total = total + point.value
                case Optional.None:
                    total = total + 0
        return total

    # The second instrument type, different Optional instance, same field-loop.
    def joined_labels(self) -> str:
        str out = ""
        for label in self.labels:
            match label.read():
                case Optional.Some(name):
                    out = out + name
                case Optional.None:
                    out = out + "_"
        return out

def main():
    # Field-list loop, mixed Some/None (base < 0 -> None).
    Meter m = Meter(
        counters=[Counter(base=1), Counter(base=-1), Counter(base=2)],
        labels=[Label(name="a"), Label(name=""), Label(name="c")],
    )
    print(m.sum_points())  # 10 + 0 + 20 = 30
    print(m.joined_labels())  # "a" + "_" + "c" = a_c

    # Regression: a LOCAL list[Counter] (already worked) still lowers and runs.
    list[Counter] locals = [Counter(base=3), Counter(base=4)]
    int64 local_total = 0
    for counter in locals:
        match counter.collect():
            case Optional.Some(point):
                local_total = local_total + point.value
            case Optional.None:
                local_total = local_total + 0
    print(local_total)  # 30 + 40 = 70

    # Regression: a LOCAL receiver (no loop) still lowers and runs.
    Counter single = Counter(base=5)
    match single.collect():
        case Optional.Some(point):
            print(point.value)  # 50
        case Optional.None:
            print(-1)

    # All-None field loop: the None arm is taken every iteration.
    Meter empty = Meter(
        counters=[Counter(base=-1), Counter(base=-2)],
        labels=[Label(name=""), Label(name="")],
    )
    print(empty.sum_points())  # 0
    print(empty.joined_labels())  # __

    # #1339: nested `Optional.Some(record)` in a constructor, present case.
    Result present = first_record(Reply(records=[Record(7), Record(8)]))
    match present.record:
        case Optional.Some(record):
            print(record.value)  # 7  (the first element)
        case Optional.None:
            print(-1)
    # #1339: absent case — an empty field list takes the `Result(None)` path.
    Result absent = first_record(Reply(records=[]))
    print(absent.record.is_some())  # False
