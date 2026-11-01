# claim: STRUCT-2
# SPEC §4.4/§7 (D-0038, I#481): a struct method honors default and keyword
# arguments exactly like a free function (I#298). Omitting a trailing defaulted
# parameter fills its default at the call site; a defaulted parameter may be
# passed explicitly or by name, and out-of-order keyword arguments bind by name.
# Defaults compose with keyword-only parameters after `*` (I#496). The identical
# defaulted parameter on a free function behaves the same. Before I#481 the
# method form reached codegen with too few arguments and crashed LLVM
# verification. Every result is rendered through `str(...)`.

struct Box:
    int64 n

    # Defaulted trailing parameter on a method — the I#481 trigger.
    def put(self, int64 a, int64 b = 0):
        self.n = a + b

    # A keyword-only parameter after `*` (I#496) that is also defaulted, to show
    # method default-filling composes with keyword-only parameters.
    def scaled(self, int64 factor, *, int64 bias = 1) -> int64:
        return self.n * factor + bias


# The same defaulted parameter on a free function — fine since I#298.
def put_fn(int64 a, int64 b = 0) -> int64:
    return a + b


def main():
    auto box = Box(0)
    box.put(5)  # omits `b` -> default 0
    print(str(box.n))  # 5
    box.put(5, 2)  # `b` passed explicitly
    print(str(box.n))  # 7
    box.put(b=3, a=4)  # out-of-order keyword arguments on a method
    print(str(box.n))  # 7
    print(str(box.scaled(2)))  # 7*2 + default bias 1 -> 15
    print(str(box.scaled(2, bias=5)))  # 7*2 + 5 -> 19
    print(str(put_fn(5)))  # free-function parity -> 5
