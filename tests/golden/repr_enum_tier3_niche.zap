# Golden fixture: a user-defined `Optional`-shaped tier-3 enum (two variants,
# one payload-free and one with a single non-nullable-pointer field) uses the
# niche layout (I#154, D-0064 #4): no tag slot, the value IS the payload
# pointer, null == the payload-free variant. Its structural render (#706) takes
# the niche path (a null test), reusing the tested niche reinterpret, so a niche
# enum NOT named `Optional` (which `optional_layout` intercepts earlier) still
# renders `Enum.Variant(field=value)` / `Enum.Empty` correctly rather than
# reading a bogus tag out of a pointer.
struct Box:
    int64 v


enum MaybeBox:
    Has(Box b)
    Nope


def main():
    print(MaybeBox.Has(Box(v=9)))  # MaybeBox.Has(b=Box(v=9))
    print(MaybeBox.Nope)  # MaybeBox.Nope
    print(str(MaybeBox.Has(Box(v=-1))))  # MaybeBox.Has(b=Box(v=-1))
    print(f"m={MaybeBox.Nope}")  # m=MaybeBox.Nope
