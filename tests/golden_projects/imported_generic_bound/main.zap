# Consumer (#1222): construct the imported generic wrapper at two distinct
# instantiations, each with its own imported conformer, and call through the
# bound after construction. Before the fix this failed native codegen with
# "'Codec' is not an interface"; now both specializations compile (static
# dispatch, no vtable) and run.
import codec


def main():
    # str specialization: TextCodec satisfies Codec[str].
    auto text = codec.Wrapper[str, codec.TextCodec](codec.TextCodec())
    print(text.decode(b"hello"))  # hello

    # int64 specialization over the SAME wrapper + bound, different conformer.
    auto length = codec.Wrapper[int64, codec.LenCodec](codec.LenCodec())
    print(length.decode(b"hello"))  # 5

    # A second str-wrapper instance: the specialization is reused, the call
    # through the bound still returns the decoded hit.
    auto again = codec.Wrapper[str, codec.TextCodec](codec.TextCodec())
    print(again.decode(b"world"))  # world
