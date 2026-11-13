# Arena v1 (#553, D-0158): a with-bound leaf byte buffer and a leaf struct
# allocated in a scope-bound region, read back within the block. Deterministic
# under both collectors.
struct Pt:
    int64 x
    int64 y

def main():
    with arena(size=4096) as a:
        auto p = a.new(Pt, 3, 4)
        auto buf = a.alloc(bytes, 8)
        print(p.x + p.y)
        print(len(buf))
