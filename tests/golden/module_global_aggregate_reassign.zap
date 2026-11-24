# Regression for the #1369 investigation: a whole reassignment (`g = value`) of
# an **aggregate-typed** module-level mutable global (D-0112) from inside a
# callee — not just `main` — persists to the caller, for both an `Optional[T]`
# payload and a reference struct. (#1369 was closed as not-a-defect: an
# aggregate global is a `ptr` slot, codegen-identical to the `list` global that
# `module_global_reassign_across_gc` already covers; the failures first blamed on
# it were the #1379 `bigint` miscompile, now fixed. This fixture pins the
# callee-reassignment path, which previously had no coverage.)

Optional[int64] g = None


struct Pt:
    int64 x


Pt gp = Pt(0)


def set_opt(int64 v):
    g = v


def set_struct(int64 v):
    gp = Pt(v)


def main():
    print(g.is_none())  # True
    set_opt(42)
    print(g.is_some())  # True — the callee's reassignment persisted
    print(g.unwrap())  # 42
    print(gp.x)  # 0
    set_struct(7)
    print(gp.x)  # 7 — the callee's struct reassignment persisted
