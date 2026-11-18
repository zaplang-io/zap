# Golden fixture: the uuid module (#975, D-0232/D-0233) — RFC 4122 / RFC 9562
# UUIDs plus NanoID. UUID generation is random, so this asserts deterministic
# properties only: the nil / max constants, the Appendix C DNS namespace, the
# canonical RFC namespaced vectors (Python's documented NAMESPACE_DNS +
# "python.org" examples), parse accepting every surface form and round-tripping,
# the version / variant nibbles, and NanoID length + a single-character
# alphabet (the one fully deterministic NanoID).
def main():
    # Nil / max: the two fixed UUIDs, plus their hex / urn / version / variant.
    print(str(uuid.nil()))
    print(str(uuid.max()))
    print(uuid.nil().hex())
    print(uuid.nil().urn())
    print(uuid.nil().version())
    print(uuid.nil().variant())
    print(uuid.max().version())
    print(uuid.max().variant())
    # The DNS namespace constant (RFC 4122 Appendix C).
    print(str(uuid.namespace_dns()))
    # RFC namespaced vectors: the exact examples from the Python uuid docs.
    print(str(uuid.uuid3(uuid.namespace_dns(), "python.org")))
    print(str(uuid.uuid5(uuid.namespace_dns(), "python.org")))
    print(uuid.uuid3(uuid.namespace_dns(), "python.org").version())
    print(uuid.uuid5(uuid.namespace_dns(), "python.org").version())
    # parse accepts the canonical, urn:uuid:, braced, and bare-32-hex forms,
    # all round-tripping to the same canonical string.
    print(str(uuid.parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8")))
    print(str(uuid.parse("urn:uuid:6ba7b810-9dad-11d1-80b4-00c04fd430c8")))
    print(str(uuid.parse("{6ba7b810-9dad-11d1-80b4-00c04fd430c8}")))
    print(str(uuid.parse("6ba7b8109dad11d180b400c04fd430c8")))
    print(uuid.parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8").version())
    print(uuid.parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8").variant())
    # uuid4: deterministic shape only (the value is random).
    auto u4 = uuid.uuid4()
    print(len(str(u4)))
    print(len(u4.hex()))
    print(len(u4.to_bytes()))
    print(u4.version())
    print(u4.variant())
    # Round-trip through parse, using the auto-derived structural equality.
    if uuid.parse(str(u4)) == u4:
        print("roundtrip-ok")
    else:
        print("roundtrip-fail")
    # uuid7 / uuid1: version + variant nibbles.
    auto u7 = uuid.uuid7()
    print(u7.version())
    print(u7.variant())
    auto u1 = uuid.uuid1()
    print(u1.version())
    print(u1.variant())
    # NanoID: default 21 chars, custom size, custom alphabet + size, empty, and
    # the single-character alphabet (the one deterministic NanoID).
    print(len(uuid.nanoid()))
    print(len(uuid.nanoid(10)))
    print(len(uuid.nanoid("abc", 8)))
    print(len(uuid.nanoid("abc", 0)))
    print(uuid.nanoid("a", 5))
