# A module `const` initializer runs exactly once, before `main` (eager init).
# The `init` line is printed by the initializer before either `main` print, and
# it appears exactly once — proving the binding is evaluated a single time.
def make() -> int64:
    print("init")
    return 42

const int64 N = make()

def main():
    print(N)
    print(N)
