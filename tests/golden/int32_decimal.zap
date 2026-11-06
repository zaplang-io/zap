# Golden fixture: an `int32` renders as a DECIMAL in every position (#709),
# fixing the symmetric half of the pre-#709 conflation where an `int32` scalar
# or struct field rendered as a *character* (because `int32` shared `Ty::I32`
# with `char` and the value renderer mapped `Ty::I32` to the char form). Now
# `char` is distinct, so `int32` flows through the integer renderer
# (`zap_i64_to_str`) as a plain decimal — as a scalar, a struct field, a tuple
# element, a list element, and a dict value.
struct Box:
    int32 n
    char c

def main():
    int32 x = 72
    print(x)  # 72
    print(str(x))  # 72
    print(f"{x}")  # 72
    # A struct with both an int32 and a char field: the int32 is a decimal, the
    # char is quoted — proving the two no longer alias.
    auto b = Box(n=65, c=char(90))
    print(b)  # Box(n=65, c='Z')
    # A tuple mixing an int32 and a char.
    auto t = (x, char(90))
    print(t)  # (72, 'Z')
    list[int32] ns = [72, 65]
    print(ns)  # [72, 65]
    dict[str, int32] d = {"k": 90}
    print(d)  # {'k': 90}
