# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; epic #661) — slice 4 (#684), SHARED HEAP OBJECT coverage.
#
# Two coroutines mutate the SAME field of a shared, plain (non-`@shareable`)
# reference `struct` with NO synchronization. A reference struct has shared
# identity (D-0023), so passing `c` to both coroutines makes them write the one
# heap object's field `n`. This is the struct-field analogue of `racy_heap_list`:
# slice 4's field-access instrumentation makes `c.n` an observed heap location.
#
# `zap run --race` MUST report the data race on the struct field with a source
# location and exit non-zero (66). Keyed on (struct header, field index) — the
# precise header is never evacuated, so the key is relocation-stable.
#
# Expected (slice 4):
#   $ zap run --race tests/race/racy_heap_struct.zap
#   -> a data-race report for a `shared heap location` (`Cell.n`); non-zero exit.
#      Without `--race` it compiles and runs (nondeterministic total).
struct Cell:
    int64 n


def bump(Cell c, bytes wg, int64 iters):
    for i in range(0, iters):
        c.n = c.n + 1
    sync.done(wg)


def main():
    Cell c = Cell(n=0)
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn bump(c, wg, 100000)
    spawn bump(c, wg, 100000)
    sync.wait(wg)
    print(c.n)
