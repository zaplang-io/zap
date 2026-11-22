# Golden fixture: net IP presentation<->wire conversion (ticket #1261,
# decisions/stdlib/1261-ip-presentation-wire.md). Pure, I/O-free parse/format
# between IP address text and its network-order octets, the stdlib equivalent of
# Python's socket.inet_pton / inet_ntop. No sockets, no DNS.
#
# Covers (exhaustive, intent documentation):
#   A. IPv4 bounds, per-octet carry-through, text->octets->text round-trip.
#   B. IPv4 rejection: out-of-range, leading zeros, wrong field count, garbage.
#   C. IPv6 full form, zero compression ("::"), canonical RFC 5952 output
#      (lowercase, longest run leftmost-on-tie, single zero NOT compressed).
#   D. IPv4-mapped (::ffff:a.b.c.d) embedded dotted form; IPv4-compatible
#      (::a.b.c.d) stays pure hex; trailing embedded IPv4 both compressed + full.
#   E. Network-order octet assertions (exact byte values).
#   F. IPv6 rejection: double "::", over-long group, wrong length, bad embedded.
#   G. Family / text mismatch (IPv6 text with ipv4 family and vice-versa).
#   H. Unknown family (both pton and ntop); case-sensitive "ipv4"/"ipv6".
#   I. inet_ntop invalid packed widths (3, 5, 15, 17 bytes).
# All rejections surface as a catchable ValueError.


# Parse `text` in `family`, format back to canonical text, and print it.
def show(str family, str text):
    print(net.inet_ntop(family, net.inet_pton(family, text)))


# A rejection probe: inet_pton(family, text) must raise.
def reject_pton(str family, str text):
    try:
        auto octets = net.inet_pton(family, text)
        print(f"NO-RAISE pton {family} [{text}] len={len(octets)}")
    except Exception:
        print(f"reject pton {family} [{text}]")


# A rejection probe: inet_ntop(family, packed) must raise.
def reject_ntop(str family, bytes packed):
    try:
        auto s = net.inet_ntop(family, packed)
        print(f"NO-RAISE ntop {family} {s}")
    except Exception:
        print(f"reject ntop {family} len={len(packed)}")


def main():
    # -- A. IPv4 round-trips (text -> octets -> text) --------------------------
    show("ipv4", "1.2.3.4")  # 1.2.3.4
    show("ipv4", "0.0.0.0")  # 0.0.0.0 (lower bound)
    show("ipv4", "255.255.255.255")  # 255.255.255.255 (upper bound)
    print(net.inet_ntop("ipv4", bytes([127, 0, 0, 1])))  # 127.0.0.1 (octets in)

    # Per-octet carry-through and width.
    auto p = net.inet_pton("ipv4", "10.20.30.40")
    assert len(p) == 4
    assert p[0] == 10 and p[1] == 20 and p[2] == 30 and p[3] == 40
    assert net.inet_pton("ipv4", "0.0.0.0") == bytes([0, 0, 0, 0])
    assert net.inet_pton("ipv4", "255.255.255.255") == bytes([255, 255, 255, 255])
    assert net.inet_pton("ipv4", "192.0.2.1") == bytes([192, 0, 2, 1])

    # -- C. IPv6 full form + zero compression (RFC 5952 canonical output) ------
    show("ipv6", "::")  # ::
    show("ipv6", "::1")  # ::1 (loopback)
    show("ipv6", "2001:db8::1")  # 2001:db8::1
    show("ipv6", "2001:0db8:0000:0000:0000:0000:0000:0001")  # 2001:db8::1 (no leading zeros)
    show("ipv6", "fe80:0:0:0:0:0:0:1")  # fe80::1
    show("ipv6", "1:2:3:4:5:6:7:8")  # 1:2:3:4:5:6:7:8 (no zeros, no "::")
    show("ipv6", "2001:DB8::1")  # 2001:db8::1 (uppercase in -> lowercase out)
    show("ipv6", "ff02::1")  # ff02::1

    # -- D. Compression rules: longest run, leftmost on tie, single not shortened
    show("ipv6", "1:0:0:0:0:0:0:8")  # 1::8 (longest run)
    show("ipv6", "1:0:0:1:0:0:1:1")  # 1::1:0:0:1:1 (leftmost of two equal runs)
    show("ipv6", "1:0:2:3:4:5:6:7")  # 1:0:2:3:4:5:6:7 (single zero stays "0")

    # -- D. IPv4-mapped vs IPv4-compatible vs full embedded IPv4 ---------------
    show("ipv6", "::ffff:192.0.2.1")  # ::ffff:192.0.2.1 (mapped -> dotted tail)
    show("ipv6", "::ffff:1.2.3.4")  # ::ffff:1.2.3.4
    show("ipv6", "0:0:0:0:0:ffff:1.2.3.4")  # ::ffff:1.2.3.4 (same address, full form in)
    show("ipv6", "::ffff:0.0.0.0")  # ::ffff:0.0.0.0
    show("ipv6", "::1.2.3.4")  # ::102:304 (IPv4-compatible stays pure hex)
    show("ipv6", "1:2:3:4:5:6:1.2.3.4")  # 1:2:3:4:5:6:102:304 (trailing v4, no "::")

    # -- E. Exact network-order octets -----------------------------------------
    assert len(net.inet_pton("ipv6", "::")) == 16
    assert net.inet_pton("ipv6", "::1") == bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])
    assert net.inet_pton("ipv6", "2001:db8::1") == bytes([0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])
    assert net.inet_pton("ipv6", "::ffff:192.0.2.1") == bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 192, 0, 2, 1])
    # inet_ntop straight from raw octets (no prior parse).
    print(
        net.inet_ntop(
            "ipv6",
            bytes([0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
        ),
    )  # 2001:db8::1

    # -- B. IPv4 rejection ------------------------------------------------------
    reject_pton("ipv4", "256.0.0.1")  # octet > 255
    reject_pton("ipv4", "01.2.3.4")  # leading zero
    reject_pton("ipv4", "1.2.3.04")  # leading zero (interior)
    reject_pton("ipv4", "1.2.3")  # too few octets
    reject_pton("ipv4", "1.2.3.4.5")  # too many octets
    reject_pton("ipv4", "1.2.3.")  # trailing dot
    reject_pton("ipv4", "1..2.3")  # empty octet
    reject_pton("ipv4", "1.2.3.4x")  # trailing garbage
    reject_pton("ipv4", "")  # empty

    # -- F. IPv6 rejection ------------------------------------------------------
    reject_pton("ipv6", ":::")  # malformed
    reject_pton("ipv6", "1::2::3")  # two "::"
    reject_pton("ipv6", "12345::1")  # group > 4 hex digits
    reject_pton("ipv6", "1:2:3:4:5:6:7:8:9")  # too many groups
    reject_pton("ipv6", "1:2:3:4:5:6:7")  # too few groups, no "::"
    reject_pton("ipv6", "1:2:3:4:5:6:7:8::")  # "::" expands zero groups
    reject_pton("ipv6", "1::2:3:4:5:6:7:8")  # "::" makes 9 groups total
    reject_pton("ipv6", "::ffff:1.2.3.256")  # bad embedded IPv4
    reject_pton("ipv6", "gggg::1")  # non-hex
    reject_pton("ipv6", "")  # empty

    # -- G. Family / text mismatch ---------------------------------------------
    reject_pton("ipv4", "2001:db8::1")  # IPv6 text, ipv4 family
    reject_pton("ipv6", "192.0.2.1")  # IPv4 text, ipv6 family

    # -- H. Unknown / case-sensitive family ------------------------------------
    reject_pton("ipv5", "1.2.3.4")  # unknown family
    reject_pton("IPV4", "1.2.3.4")  # case-sensitive: "IPV4" != "ipv4"
    reject_ntop("ipv5", bytes([1, 2, 3, 4]))  # unknown family on ntop

    # -- I. inet_ntop invalid packed widths ------------------------------------
    reject_ntop("ipv4", bytes([1, 2, 3]))  # 3 bytes
    reject_ntop("ipv4", bytes([1, 2, 3, 4, 5]))  # 5 bytes
    reject_ntop("ipv6", net.inet_pton("ipv6", "::")[:15])  # 15 bytes
    reject_ntop("ipv6", net.inet_pton("ipv6", "::") + bytes([0]))  # 17 bytes

    print("ok")
