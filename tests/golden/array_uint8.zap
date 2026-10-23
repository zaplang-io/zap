# Golden fixture: uint8 arrays (ticket #49) — the image-data dtype.
# Elements are 1-byte unsigned words; element-wise arithmetic wraps mod 256
# (D-0005 wrap). A weak integer scalar keeps the dtype (`a * 2` stays uint8).
# Indexing and sum cross the ABI as i64 (zero-extended / accumulated wide).
def main():
    auto a = array[uint8]([10, 20, 30, 40, 250])
    auto b = array[uint8]([1, 2, 3, 4, 10])
    print(sum(a + b))  # 11,22,33,44,(260 % 256 = 4) -> 114
    print(sum(a * 2))  # 20,40,60,80,(500 % 256 = 244) -> 444
    print(sum(a))  # 10+20+30+40+250 = 350 (accumulates in i64)
    print(a[4])  # 250 (get zero-extends the byte)
    print(len(a))  # 5
