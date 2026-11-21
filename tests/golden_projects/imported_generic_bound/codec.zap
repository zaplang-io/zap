# Library module (#1222): a parameterized interface `Codec[T]`, two concrete
# conformers over distinct decoded types, and a generic wrapper whose second
# type parameter is bounded by the interface and references the first
# (`C: Codec[T]`). This is the exact shape of the typed serde wrapper in
# zaplang-io/memcached#8/#21. Declared in an imported module so the linker
# mangles the interface, its conformers, and the wrapper with the module prefix
# — the bound's interface name must be mangled with them, or the monomorphizer
# reports "'Codec' is not an interface".
interface Codec[T]:
    def decode(self, bytes raw) -> T


struct TextCodec:
    def decode(self, bytes raw) -> str:
        return str(raw)


struct LenCodec:
    def decode(self, bytes raw) -> int64:
        return len(raw)


struct Wrapper[T, C: Codec[T]]:
    C codec

    # The call THROUGH the bound: `self.codec` has the bounded type `C`, and
    # `.decode` resolves through `Codec[T]`, yielding `T`.
    def decode(self, bytes raw) -> T:
        return self.codec.decode(raw)
