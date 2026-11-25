# Regression for #1487: a user binding named `reflect` must not be hijacked by
# the `reflect.construct(T)` module special-case. That rewrite keyed only on the
# receiver being literally named `reflect`, without checking (as the sibling
# `datetime.zone_abbreviation` special-case does) that the name is unshadowed —
# so a local/parameter/field named `reflect` whose type declares its own
# `construct` failed monomorphization with
# `error[ZAP6001]: reflect.construct(T) expects a type name` instead of
# dispatching to the user method. Covers a local, a parameter, and a field named
# `reflect`, each calling its own `construct`.


struct Thing:
    int64 n

    def construct(self, int64 x) -> int64:
        return x + self.n


struct Holder:
    Thing reflect


# A parameter named `reflect`.
def via_param(Thing reflect) -> int64:
    return reflect.construct(5)


# A field named `reflect` reached through `self`.
def via_field(Holder h) -> int64:
    return h.reflect.construct(7)


def main():
    # A local named `reflect`.
    auto reflect = Thing(10)
    print(reflect.construct(5))  # 15

    print(via_param(Thing(100)))  # 105

    auto holder = Holder(Thing(20))
    print(via_field(holder))  # 27
