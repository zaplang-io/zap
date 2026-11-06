# Golden fixture: a user struct with NO __str__/__repr__/@derive(Repr) now gets
# the general Display protocol's default field-wise render `TypeName(field=value,
# ...)` across print(), str(), and f-strings identically (#274, D-0124). Before
# #274 this was a compile error in print() and str()/f-string rejected the struct
# outright.
struct Point:
    int64 x
    int64 y

def main():
    auto p = Point(x=1, y=2)
    print(p)  # Point(x=1, y=2)
    print(str(p))  # str() renders identically
    print(f"pt={p}")  # f-string field renders identically
