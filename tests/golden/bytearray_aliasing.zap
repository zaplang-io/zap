# Golden fixture: the bytearray no-aliasing guarantee (#1385).
#
# bytearray(b) COPIES into a fresh buffer — mutating the result must never write
# the source `bytes`. bytes(ba) likewise snapshots, and bytearray(ba) copies, so
# later mutation of one never changes the other.
def main():
    auto src = b"abc"
    auto a = bytearray(src)
    a[0] = 90  # Z
    a.append(33)  # !
    print(str(src))  # abc   (source bytes unchanged)
    print(str(bytes(a)))  # Zbc!

    # bytes(bytearray) snapshots: mutating the bytearray afterwards does not
    # change the returned bytes.
    auto snap = bytes(a)
    a[0] = 65  # A
    a.append(63)  # ?
    print(str(snap))  # Zbc!  (snapshot unchanged)
    print(str(bytes(a)))  # Abc!?

    # bytearray(bytearray) copies: the copy is independent of the original.
    auto b = bytearray(a)
    b[0] = 88  # X
    print(str(bytes(a)))  # Abc!?  (original unchanged)
    print(str(bytes(b)))  # Xbc!?
