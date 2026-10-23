# @derive(Json) with Optional[T] fields (I#165, D-0065). An Optional field is
# the way to say "nullable / may be absent": decode maps an absent key OR an
# explicit JSON null to None, and a present value to Some(v); dumps omits a None
# key entirely. Covers a scalar payload (Optional[int64], tagged layout) and a
# pointer payload (Optional[str], niche layout). Round-trips prove absent->None
# and present->Some. (Booleans are checked via if/else to keep the output free of
# bool-rendering concerns.)

@derive(Json)
struct Profile:
    str name
    Optional[int64] age
    Optional[str] nick

def main():
    # Present: both optionals decode to Some.
    auto p = Profile.from_json(
        "{\"name\": \"ada\", \"age\": 36, \"nick\": \"countess\"}",
    )
    print(p.name)  # ada
    if p.age.is_some():
        print(p.age.unwrap())  # 36
    print(p.nick.unwrap())  # countess
    print(p.to_json())  # {"name":"ada","age":36,"nick":"countess"}

    # Absent keys: both optionals decode to None; dumps omits them.
    auto q = Profile.from_json("{\"name\": \"bob\"}")
    if q.age.is_none():
        print("age-none")  # age-none
    if q.nick.is_none():
        print("nick-none")  # nick-none
    print(q.to_json())  # {"name":"bob"}

    # Explicit JSON null also decodes to None (dumps still omits).
    auto r = Profile.from_json("{\"name\": \"cas\", \"age\": null, \"nick\": null}")
    if r.age.is_none():
        print("r-age-none")  # r-age-none
    if r.nick.is_none():
        print("r-nick-none")  # r-nick-none
    print(r.to_json())  # {"name":"cas"}

    # Mixed: one present (scalar), one absent (pointer).
    auto s = Profile.from_json("{\"name\": \"dot\", \"age\": 7}")
    print(s.age.unwrap())  # 7
    if s.nick.is_none():
        print("s-nick-none")  # s-nick-none
    print(s.to_json())  # {"name":"dot","age":7}
