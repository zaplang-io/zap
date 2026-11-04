# Reassigning a module-level `const` is a compile error (ZAP3017): it is
# immutable (D-0003/D-0112).
const int64 N = 1

def main():
    N = 2
    print(N)
