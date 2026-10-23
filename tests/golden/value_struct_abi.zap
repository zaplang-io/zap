# Golden fixture: D-0051 physical inline / register / sret calling convention.
# A flat `value struct` is passed and returned BY VALUE (no heap box for the
# pass): small ones (<= 2 machine words) travel in registers, larger ones via the
# `sret` struct-return-pointer convention the backend inserts. The observable
# value semantics are unchanged — a value struct is still copied on pass/return
# and on assignment, so a callee's (or a copy's) mutation never escapes.

# Register-sized: two f64 fields fit in registers.
value struct Vec2:
    f64 x
    f64 y

# sret-sized: four int64 fields (32 bytes) are returned via an sret pointer.
value struct Quad:
    int64 a
    int64 b
    int64 c
    int64 d

def mk_vec(f64 x, f64 y) -> Vec2:
    return Vec2(x=x, y=y)

# Mutates its own by-value copy, then returns it; the caller's argument is
# untouched (pass-by-value).
def bump_vec(Vec2 v) -> Vec2:
    v.x = v.x + 1.0
    return v

def mk_quad(int64 a, int64 b, int64 c, int64 d) -> Quad:
    return Quad(a=a, b=b, c=c, d=d)

def sum_quad(Quad q) -> int64:
    return q.a + q.b + q.c + q.d

def main():
    auto v = mk_vec(3.0, 4.0)
    print(v.x)  # 3.0  (returned in registers)
    print(v.y)  # 4.0
    auto w = bump_vec(v)  # callee bumps its own copy and returns it
    print(w.x)  # 4.0
    print(v.x)  # 3.0  — caller's v unchanged by the by-value pass

    auto q = mk_quad(1, 2, 3, 4)  # returned via sret
    print(sum_quad(q))  # 10
    auto r = q  # value copy (independent instance)
    r.a = 100  # mutate the copy only
    print(sum_quad(q))  # 10   — original unaffected
    print(sum_quad(r))  # 109
