# Eager init in source order (D-0112): a mutable global's initializer reads an
# earlier module-level binding — a `const` and a mutable one — and the chain of
# dependent initializers runs once, before `main`, in declaration order.
const int64 BASE = 10
int64 derived = BASE * 2
int64 tripled = derived + BASE

def main():
    print(BASE)  # 10
    print(derived)  # 20
    print(tripled)  # 30
