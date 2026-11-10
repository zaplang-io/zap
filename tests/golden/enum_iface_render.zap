# Golden fixture: Display rendering of an interface value backed by an enum
# (#731 x #707, D-0016/D-0124). Before #731 a tier-1/tier-3 enum could not sit
# directly behind an interface, so #707 could only exercise the enum renderers
# through the interface as a struct FIELD (see repr_iface_fields). Now the
# interface value is constructible directly, so `print`/`str`/f-string on it
# dynamically dispatches through the itable's renderer slot to the enum's own
# renderer: a tier-3 enum's structural form, and — via a synthesized box renderer
# that reloads the tag — a tier-1 enum's `Enum.Member` form.

interface Node:
    def rank(self) -> int

enum Leaf:  # tier-1
    A
    B

    def rank(self) -> int:
        return 0

enum Tree:  # tier-3
    Branch(int w)
    Twig

    def rank(self) -> int:
        return 0

def show(Node n):
    print(n)

def main():
    # Tier-1 enum behind an interface renders as `Leaf.Member`.
    Node a = Leaf.B
    print(a)  # Leaf.B
    print(str(a))  # Leaf.B
    print(f"[{a}]")  # [Leaf.B]

    # Tier-3 enum behind an interface renders structurally.
    Node b = Tree.Branch(w=9)
    print(b)  # Tree.Branch(w=9)
    print(f"[{b}]")  # [Tree.Branch(w=9)]
    Node c = Tree.Twig
    print(c)  # Tree.Twig  (payload-free: no parentheses)

    # Render follows the concrete value behind the fat pointer, dynamically,
    # through an interface-typed parameter and across both tiers.
    show(Leaf.A)  # Leaf.A
    show(Tree.Branch(w=1))  # Tree.Branch(w=1)
