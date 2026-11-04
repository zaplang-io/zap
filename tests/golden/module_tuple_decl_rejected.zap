# A module-level tuple-destructuring declaration is NOT a single-name binding,
# so it is not admitted at module scope (ZAP3018, D-0112): declare each binding
# on its own line. Only `const T NAME = expr` and `T NAME = expr` are allowed.
auto a, b = (1, 2)

def main():
    print(a + b)
