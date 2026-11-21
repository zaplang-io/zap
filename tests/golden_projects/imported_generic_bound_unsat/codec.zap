# Library module (#1222, rejection case): the same parameterized interface and
# bounded generic wrapper, plus an imported type that does NOT implement the
# interface (no `decode` method). With the bound's interface name now mangled
# across the import boundary, the monomorphizer resolves the bound to the real
# imported interface and correctly rejects the non-conformer — rather than
# failing with "'Codec' is not an interface".
interface Codec[T]:
    def decode(self, bytes raw) -> T


struct NotACodec:
    int64 tag


struct Wrapper[T, C: Codec[T]]:
    C codec

    def decode(self, bytes raw) -> T:
        return self.codec.decode(raw)
