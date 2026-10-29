# Golden fixture: xml build + dumps + round-trip determinism (I#351, D-0083).
# Builds a tree with the builder API, serializes it, reparses, and serializes
# again — the two serializations must be byte-identical. Also covers self-closing
# empty elements and text/attribute escaping.
def main():
    auto root = xml.element("Config")
    root.set_attr("xmlns", "urn:s3")
    auto name = xml.element("Name")
    name.set_text("my-bucket")
    root.add_child(name)
    # An element with no text and no children serializes self-closing.
    root.add_child(xml.element("Versioning"))

    auto s1 = xml.dumps(root)
    print(s1)  # <Config xmlns="urn:s3"><Name>my-bucket</Name><Versioning/></Config>

    # Deterministic round-trip: dumps(parse(dumps(tree))) == dumps(tree).
    auto s2 = xml.dumps(xml.parse(s1))
    if s2 == s1:
        print("roundtrip-ok")

    # Text escaping: & < > are escaped; quotes are left as-is in text.
    auto m = xml.element("Msg")
    m.set_text("a < b & c > d")
    auto ms = xml.dumps(m)
    print(ms)  # <Msg>a &lt; b &amp; c &gt; d</Msg>
    print(xml.parse(ms).get_text())  # a < b & c > d

    # Attribute escaping: & < " are escaped inside the quoted value.
    auto e = xml.element("E")
    e.set_attr("v", "x<y&z\"w")
    auto es = xml.dumps(e)
    print(es)  # <E v="x&lt;y&amp;z&quot;w"/>
    print(xml.parse(es).get_attr("v").unwrap())  # x<y&z"w

    # set_attr replaces an existing attribute (no duplicate).
    auto d = xml.element("D")
    d.set_attr("k", "1")
    d.set_attr("k", "2")
    print(xml.dumps(d))  # <D k="2"/>
