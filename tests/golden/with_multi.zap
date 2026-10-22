# Golden (I#185): `with a, b:` nests left-to-right (Python). Both `__enter__`s run
# in order (a then b), and on exit `__exit__`s run in REVERSE (b then a). Proves
# multi-manager support and ordering.
struct Guard:
    str name
    def __enter__(self) -> Guard:
        print("enter " + self.name)
        return self
    def __exit__(self):
        print("exit " + self.name)

def main():
    with Guard("a"), Guard("b"):
        print("body")
