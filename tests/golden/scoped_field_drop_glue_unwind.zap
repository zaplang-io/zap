# Golden (#876, D-0159): the `@scoped` drop-glue runs on the EXCEPTION-UNWIND exit
# path too, not just normal fall-through. An exception raised inside the composite
# `with` body unwinds out; the owner's `__exit__` runs, then the owned fields'
# `__exit__` in reverse declaration order, then the exception propagates to the
# caller's handler ("after" is never printed).
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
        return self
    def __exit__(self):
        print("exit pair")

def risky():
    with Pair() as p:
        print("before")
        raise "boom"
        print("after")

def main():
    try:
        risky()
    except Exception as e:
        print("caught")
