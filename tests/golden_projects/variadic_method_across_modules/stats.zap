# A library module exporting a struct whose method takes a variadic parameter
# (#1218, D-0038). The method body is lowered in this module; `main` calls it
# across the module boundary, so the collected-list argument is assembled at a
# call site in a different module from the callee.
struct Client:
    int64 base

    def record(self, bytes... keys) -> int64:
        return self.base + len(keys)


def make(int64 b) -> Client:
    return Client(base=b)
