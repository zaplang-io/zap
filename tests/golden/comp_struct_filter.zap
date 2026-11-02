# Golden fixture (#573): a list comprehension that filters structs by a field
# in the `if` guard — `[p for p in ps if p.age > 18]` over a `list[P]`. A
# language review reported this ICEd codegen (struct-field access inside a
# comprehension guard). This guards that path and also reads a field off the
# filtered elements so the struct values, not just the count, are exercised.
# Input order is preserved by the comprehension, so the output is deterministic.
struct P:
    int64 age

def main():
    list[P] ps = [P(age=10), P(age=20), P(age=30)]
    auto adults = [p for p in ps if p.age > 18]
    print(len(adults))  # 2
    print(adults[0].age)  # 20
    print(adults[1].age)  # 30
