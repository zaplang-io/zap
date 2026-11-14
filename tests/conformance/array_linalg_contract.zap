# claim: ARR-17
# SPEC §15 (#926): `import linalg` provides LAPACK-backed linear algebra over
# numeric arrays — solve(a, b) (a @ x == b), inv(a), det(a). `a` must be a
# square 2-D matrix; integer input promotes to f64; a singular matrix / bad
# shape raises a catchable ValueError. Results are f64.
import linalg

def main():
    print("det", str(linalg.det(array[f64]([[2.0, 0.0], [0.0, 4.0]]))))  # 8.0
    auto x = linalg.solve(array[f64]([[2.0, 0.0], [0.0, 4.0]]), array[f64]([6.0, 8.0]))
    print("solve", str(x[0]), str(x[1]))  # 3.0 2.0
    auto iv = linalg.inv(array[f64]([[2.0, 0.0], [0.0, 4.0]]))
    print("inv", str(iv[0, 0]), str(iv[1, 1]))  # 0.5 0.25
    try:
        print(str(linalg.det(array[f64]([[1.0, 2.0, 3.0]]))))
    except ValueError:
        print("nonsquare raises")
