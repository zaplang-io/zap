# Golden (compile error): a bit width on a plain (reference) struct field is
# rejected. C-style bitfields (I#173, D-0068) are a value-struct-only layout
# feature — a reference struct (D-0023) is a GC pointer object, never a packed
# scalar blob. The driver exits non-zero and explains why (see the .stderr
# sibling); stdout stays empty.
struct Bad:
    int64 flags: 3

def main():
    print(0)
