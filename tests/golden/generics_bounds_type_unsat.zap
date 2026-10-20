# Golden (compile error): an unsatisfied bound on a generic TYPE is rejected
# (I#164). `Box[T: Display]` requires its element to provide `show`; `Mute` does
# not, so `Box[Mute]` fails the bound at monomorphization. The driver exits
# non-zero and names the type, bound, and parameter (see the .stderr sibling).

interface Display:
    def show(self) -> str

struct Mute:
    int64 n

struct Box[T: Display]:
    T item
    def reveal(self) -> str:
        return self.item.show()

def main():
    Box[Mute] b = Box(Mute(5))
    print(b.reveal())
