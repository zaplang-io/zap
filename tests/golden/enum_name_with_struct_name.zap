# Golden fixture: the enum `.name` disambiguation (I#141) must not regress enums.
# Enums keep working next to a struct that has a `name` field.
enum Color:
    RED
    GREEN

struct Tag:
    str name

def main():
    auto t = Tag(name="x")
    print(t.name)           # x
    print(int64(Color.GREEN))  # 1
