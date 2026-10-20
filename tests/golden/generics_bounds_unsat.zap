# Golden (compile error): an unsatisfied interface bound is rejected (I#164).
#
# `Mute` has no `show` method, so instantiating the `Display`-bounded function
# `announce` at `Mute` fails the bound at monomorphization — a static,
# compile-time error with no runtime cost. The driver exits non-zero and names
# the offending type, bound, and parameter (see the .stderr sibling).

interface Display:
    def show(self) -> str

struct Mute:
    int64 n

def announce[T: Display](T x) -> str:
    return x.show()

def main():
    print(announce(Mute(5)))
