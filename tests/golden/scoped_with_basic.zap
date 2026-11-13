# Golden (#876, D-0159): a bare `@scoped` struct used as a `with` head runs the
# context-manager protocol exactly like any other manager — `__enter__` on entry,
# body, `__exit__` on exit. It owns no `@scoped` field, so the auto-teardown
# drop-glue list is empty and lowering is unchanged from an ordinary `with`.
@scoped
struct Res:
    int64 id = 0
    def __enter__(self) -> Res:
        print("enter")
        return self
    def __exit__(self):
        print("exit")

def main():
    with Res() as r:
        print(r.id)
