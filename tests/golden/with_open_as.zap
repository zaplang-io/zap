# Golden (I#185): the general context-manager protocol on a user struct, with
# `with EXPR as NAME:`. `__enter__` runs on entry and its result is bound to `f`
# (typed as File, so `f.read()` type-checks — D-0069 Q4); `__exit__` runs on
# normal exit. Proves `as` binding + __enter__-before-body + __exit__-after over
# a real file written to a temp dir (output independent of the temp path).
struct File:
    str path
    def __enter__(self) -> File:
        print("open")
        return self
    def read(self) -> str:
        return fs.read_file(self.path)
    def __exit__(self):
        print("close")

def main():
    auto d = fs.mkdtemp()
    auto p = os.path_join(d, "hello.txt")
    fs.write_file(p, "hello world")
    with File(p) as f:
        print(f.read())
    os.remove(p)
    os.rmdir(d)
