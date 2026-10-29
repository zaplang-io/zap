# Golden fixture: xml.parse + navigation (I#351, D-0083). Covers nested
# elements, attributes, default-namespace inheritance, prefixed names /
# local_name, text content, and entity decoding.
def main():
    auto doc = "<Root xmlns=\"urn:ex\"><Item id=\"1\"><Name>Alice</Name></Item><Item id=\"2\"><Name>Bob</Name></Item></Root>"
    auto root = xml.parse(doc)
    print(root.name)  # Root
    print(root.namespace)  # urn:ex

    # First Item: attribute read + inherited namespace + nested text.
    auto first = root.find("Item").unwrap()
    print(first.get_attr("id").unwrap())  # 1
    print(first.namespace)  # urn:ex
    print(first.find("Name").unwrap().get_text())  # Alice

    # find_all returns every matching child (wrapped-list navigation).
    auto items = root.find_all("Item")
    print(len(items))  # 2
    for it in items:
        print(it.find("Name").unwrap().get_text())  # Alice, then Bob

    # A missing child / attribute is None, not a crash.
    if root.find("Nope").is_none():
        print("no-nope")

    # Prefixed names: local_name strips the prefix; the URI resolves from the
    # prefix binding and is inherited by the child.
    auto pdoc = "<r:Root xmlns:r=\"urn:r\"><r:Child>hi</r:Child></r:Root>"
    auto pr = xml.parse(pdoc)
    print(pr.name)  # r:Root
    print(pr.local_name())  # Root
    print(pr.namespace)  # urn:r
    auto ch = pr.find("r:Child").unwrap()
    print(ch.local_name())  # Child
    print(ch.namespace)  # urn:r

    # Entity decoding in text and attribute values.
    auto edoc = "<E a=\"x&lt;y&amp;z\">1 &lt; 2 &amp; 3 &#65; &#x42;</E>"
    auto e = xml.parse(edoc)
    print(e.get_attr("a").unwrap())  # x<y&z
    print(e.get_text())  # 1 < 2 & 3 A B
