# Golden fixture: the `bytes` value type (ticket #337).
#
# Construction (b"..." literal, bytes(str), bytes(list[int]), bytes()), len,
# byte indexing (-> int64 in 0..=255), slicing, concatenation, equality, and the
# str<->bytes round-trip (UTF-8 encode/decode).
def main():
    auto b = b"hello"
    print(len(b))  # 5
    print(b[0])  # 104
    print(b[4])  # 111
    print(b[-1])  # 111
    print(str(b[1:4]))  # ell
    print(len(b[1:4]))  # 3
    print(str(b[::2]))  # hlo
    auto from_str = bytes("hi")
    print(len(from_str))  # 2
    print(from_str[0])  # 104
    auto from_list = bytes([104, 105])
    print(str(from_list))  # hi
    print(str(from_str == from_list))  # True
    print(str(b == from_str))  # False
    print(str(b"ab" + b"cd"))  # abcd
    print(len(bytes()))  # 0
    print(str(bytes("café")))  # café  (UTF-8 round-trip)
    print(str(b"café" == bytes("café")))  # True
