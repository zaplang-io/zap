# Golden fixture: an interface-typed STRUCT FIELD holding an enum (#731,
# D-0016). A struct field typed as an interface stores the `{data, itable}` fat
# pointer; constructing the struct boxes the concrete enum into it, and a method
# call through the field dispatches dynamically. Both tiers are exercised: a
# tier-1 enum (boxed scalar) and a tier-3 enum (boxed tagged struct) as the same
# interface-typed field. The struct also renders, so the field's enum shows
# through the interface value (ties to #707).

interface Node:
    def rank(self) -> int

enum Leaf:  # tier-1
    A
    B

    def rank(self) -> int:
        match self:
            case Leaf.A:
                return 1
            case _:
                return 2

enum Tree:  # tier-3
    Branch(int w)

    def rank(self) -> int:
        match self:
            case Tree.Branch(w):
                return w

struct Holder:
    Node item

def main():
    Holder h1 = Holder(item=Leaf.B)
    print(h1.item.rank())  # 2  (dispatch through an interface-typed field)

    Holder h2 = Holder(item=Tree.Branch(w=9))
    print(h2.item.rank())  # 9

    # Render the holder: the interface-typed field dispatches to the enum's own
    # renderer behind the fat pointer.
    print(h1)  # Holder(item=Leaf.B)
    print(h2)  # Holder(item=Tree.Branch(w=9))
