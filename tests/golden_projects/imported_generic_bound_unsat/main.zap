# Consumer (#1222, rejection case): instantiate the imported wrapper with an
# imported type that does not satisfy the imported `Codec[str]` bound. The
# compile must fail with a clear "does not satisfy the bound" diagnostic naming
# the missing method — proving the bound resolves to the real imported
# interface instead of being lost as an unqualified name.
import codec


def main():
    auto w = codec.Wrapper[str, codec.NotACodec](codec.NotACodec(0))
    print(w.decode(b"hello"))
