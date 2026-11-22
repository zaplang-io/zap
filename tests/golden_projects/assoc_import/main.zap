# Associated functions through an import (#1260): `pkg.Type.func(...)`. Modlink
# rewrites `msg.Message` to the imported struct's mangled name, so the associated
# dispatch resolves `msg.Message.query(...)` exactly like a local
# `Message.query(...)` — the acceptance criterion that the type may be imported
# from a package.
import msg

def main():
    auto a = msg.Message.query(42)
    print(a.id)  # 42

    # Keyword argument through the import.
    auto b = msg.Message.query(id=99)
    print(b.id)  # 99

    # Alternate constructor through the import; instance method on the result.
    auto c = msg.Message.from_wire(bytes([7, 1, 2]))
    print(c.id)  # 7
    print(len(c.to_wire()))  # 3
