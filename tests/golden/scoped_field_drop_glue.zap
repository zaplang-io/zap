# Golden (#876, D-0159): auto-teardown drop-glue. `Pair` is `@scoped` and owns two
# `@scoped` `Res` fields, moved into place in its `__enter__`. At `with`-exit the
# compiler runs the owner's own `__exit__` FIRST, then each owned field's
# `__exit__` in REVERSE declaration order (b before a) — Rust `Drop`-glue /
# `ExitStack` semantics — so an owned scoped field can never be forgotten even
# though the author hand-writes `__exit__`. An owned field's `__enter__` is NOT
# auto-run (only `__exit__` is drop-glued); each `Res.__exit__` prints its id.
@scoped
struct Res:
    int64 id = 0
    def __enter__(self) -> Res:
        return self
    def __exit__(self):
        print(self.id)

@scoped
struct Pair:
    Res a
    Res b
    def __enter__(self) -> Pair:
        self.a = Res(1)
        self.b = Res(2)
        print("enter pair")
        return self
    def __exit__(self):
        print("exit pair")

def main():
    with Pair() as p:
        print("body")
