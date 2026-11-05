# Value-struct module globals (#689; D-0051 value semantics): a const and a
# mutable `value struct` global both support field reads and method calls. (Only
# reads are exercised here — field *mutation* through a value-struct global slot
# is a separate write-path concern, not the member-resolution fix of #689.)
value struct Vec2:
    int64 x
    int64 y

    def sum(self) -> int64:
        return self.x + self.y

const Vec2 ORIGIN = Vec2(1, 2)
Vec2 cur = Vec2(3, 4)

def main():
    print(ORIGIN.x)  # 1  — const value-struct global field read
    print(ORIGIN.y)  # 2
    print(ORIGIN.sum())  # 3  — const value-struct global method call
    print(cur.x)  # 3  — mutable value-struct global field read
    print(cur.sum())  # 7  — mutable value-struct global method call
