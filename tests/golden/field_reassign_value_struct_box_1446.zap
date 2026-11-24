# Golden fixture: reassigning a *returned* `value struct` to an ALREADY-EXISTING
# aggregate field (`obj.field = f()`) must keep a valid value after the assigning
# frame returns (zaplang-io/zap#1446).
#
# This is the one field-store site #1400 (PR #1408) did NOT cover: #1400 boxed
# value structs stored during construction (`Struct(field=f())`), into tagged
# enum-variant payloads, and the `#1262` `Optional.Some` auto-wrap, but the
# field-ASSIGNMENT statement arm (`crates/ir/src/lower/stmts.rs`) still used the
# lvalue-only `copy_if_value`. A returned (non-lvalue) inline-eligible value
# struct is physically a register/`sret` STACK spill (D-0051); storing that
# spill's pointer straight into an existing field left the field dangling once
# the producer's frame returned, so a later read came back corrupted. The fix
# routes the field-assignment arm through the same `copy_field_value` chokepoint,
# boxing the value struct onto the heap.
#
# This fixture drives the reported shape end to end (a method caching a returned
# `DateTime` seed) plus a general, non-datetime value struct via both a method
# and a free function, with an overwrite and a GC-churning allocation between the
# store and the read, so a regression cannot silently reintroduce the dangling
# store.
import datetime


# --- The issue's exact repro: a cursor caching a returned DateTime seed. ---
# `emit` caches `self.seed = self._make_seed()` on the first call, then reads it
# back on later calls to produce additional times on the same day. Before the
# fix the second emit read a corrupted `0000-00-00T02:00:00`.
struct _ProbeCursor:
    DateTime seed
    int64 index = 0

    def _make_seed(self) -> DateTime:
        return datetime.combine(datetime.date(2024, 1, 31), datetime.time(0, 0, 0))

    def emit(self) -> DateTime:
        if self.index == 0:
            self.seed = self._make_seed()
        self.index += 1
        return datetime.replace(self.seed, hour=self.index, nanosecond=0)


# --- A general (non-datetime) inline-eligible value struct. ---
value struct Seed:
    str tag
    int64 n


# A method caches a returned Seed in a field, then reads it back after a later
# allocation (a GC safepoint) — the general twin of the DateTime cursor.
struct GenCursor:
    Seed seed
    int64 index = 0

    def fresh(self) -> Seed:
        return Seed(tag="gen", n=100 + self.index)

    def advance(self):
        self.seed = self.fresh()
        self.index += 1


# A free function (not a method) returning a value struct by value.
def remake(str tag, int64 n) -> Seed:
    return Seed(tag=tag, n=n)


# Force heap churn / a GC safepoint so a dangling field would be reused/zeroed
# before the read — returns a running sum so the work is not optimized away.
def churn() -> int64:
    int64 acc = 0
    auto xs = [0]
    for i in range(0, 256):
        xs.append(i)
        acc = acc + xs[len(xs) - 1]
    return acc


struct Holder:
    Seed slot


def main():
    # === The reported DateTime cursor ===
    auto cursor = _ProbeCursor(datetime.from_timestamp(0))
    auto first = cursor.emit()
    print(datetime.isoformat(first))  # 2024-01-31T01:00:00
    auto second = cursor.emit()
    print(datetime.isoformat(second))  # 2024-01-31T02:00:00

    # === General value struct cached by a method, read after allocation ===
    auto gc = GenCursor(Seed(tag="init", n=0))
    gc.advance()
    auto churned = churn()
    # The field must still own a valid Seed after `advance` returned and after a
    # GC-churning allocation — not a dangling stack spill.
    print(gc.seed.tag)  # gen
    print(gc.seed.n)  # 100
    print(churned)  # 32640

    # A second advance produces an independent Seed; the first is fully replaced.
    gc.advance()
    print(gc.seed.tag)  # gen
    print(gc.seed.n)  # 101

    # === Free-function result reassigned to an existing field, with overwrite ===
    auto holder = Holder(remake("start", -1))
    holder.slot = remake("alpha", 11)
    print(holder.slot.tag)  # alpha
    print(holder.slot.n)  # 11

    # Overwrite the field; the NEW value must be intact across a GC safepoint.
    holder.slot = remake("omega", 22)
    auto more = churn()
    print(holder.slot.tag)  # omega
    print(holder.slot.n)  # 22
    print(more)  # 32640
