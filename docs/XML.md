# The `xml` module

`xml` is Zap's core XML DOM: parse a document into a navigable tree, and
serialize a tree back out with **stable, byte-deterministic** output. It is
specced for the AWS **rest-xml** (S3) and **query** (STS/SNS/IAM/EC2) wire
protocols, but it is a general, dependency-free XML library.

The module gives you the XML *primitives*. It knows nothing about AWS shapes —
higher layers map shape traits (`xmlAttribute`, flattened lists, `locationName`,
…) onto these primitives. The design rationale lives in
[`design/0033-xml.md`](../design/0033-xml.md); this page is the usage guide.

> **Status:** Experimental / additive (STABILITY §4.2). The module carries text
> only — timestamp formatting, base64 blobs and percent-encoding are separate
> codec concerns and are **not** here.

## The DOM

Two reference structs ship in the prelude:

```zap
struct XmlAttr:
    str name       # "xmlns", "xmlns:ns", "ns:attr" or a bare "attr"
    str value      # entity-decoded

struct XmlElement:
    str name                    # tag as written (may be prefixed: "ns:Foo")
    str namespace               # resolved namespace URI, "" if none
    str text                    # concatenated direct text, entity-decoded
    list[XmlAttr] attrs         # attributes in document order (incl. xmlns decls)
    list[XmlElement] children   # child elements in document order
```

Each element has one `text` field and a list of child elements — the data model
AWS protocols use. True mixed content (text interleaved *between* child elements
with order significant) is flattened; this never arises in AWS payloads.

## Parsing and navigating

```zap
auto doc = "<Root xmlns=\"urn:ex\"><Item id=\"1\"><Name>Alice</Name></Item></Root>"
auto root = xml.parse(doc)

print(root.name)                              # Root
print(root.namespace)                         # urn:ex

auto item = root.find("Item").unwrap()        # first child named "Item"
print(item.get_attr("id").unwrap())           # 1
print(item.namespace)                         # urn:ex  (inherited)
print(item.find("Name").unwrap().get_text())  # Alice
```

Navigation methods on `XmlElement`:

| method | returns | |
|--------|---------|--|
| `find(name)` | `Optional[XmlElement]` | first child with that tag name |
| `find_all(name)` | `list[XmlElement]` | **all** children with that tag name |
| `get_attr(name)` | `Optional[str]` | attribute value by name |
| `get_text()` | `str` | the element's text content |
| `local_name()` | `str` | `name` with any `prefix:` stripped |

A missing child or attribute returns `Optional.None` — no crash:

```zap
if root.find("Missing").is_none():
    print("not present")
```

`xml.parse` accepts `str` or `bytes` and returns the **root element**.

## Building and serializing

```zap
auto cfg = xml.element("CreateBucketConfiguration")
auto loc = xml.element("LocationConstraint")
loc.set_text("us-west-2")
cfg.add_child(loc)

print(xml.dumps(cfg))
# <CreateBucketConfiguration><LocationConstraint>us-west-2</LocationConstraint></CreateBucketConfiguration>
```

Builder helpers (each returns `self`, so calls chain):

| method | |
|--------|--|
| `xml.element(name)` | a fresh empty element |
| `set_text(t)` | set text content |
| `set_attr(name, value)` | set/replace an attribute |
| `add_child(c)` | append a child element |

`xml.dumps` is **deterministic**: attributes and children serialize in list
order, with no sorting pass — same tree, identical bytes, every run. An element
with neither text nor children serializes self-closing (`<Name/>`). Round-trip:
`dumps(parse(dumps(tree))) == dumps(tree)`.

Escaping is automatic: text escapes `& < >`; attribute values escape `& < "`.
Parsing decodes `&lt; &gt; &amp; &quot; &apos;` and numeric `&#DD;` / `&#xHH;`,
and treats `<![CDATA[…]]>` as literal text.

## Expressing AWS shape features

Everything the AWS codegen layer needs maps onto the primitives above:

```zap
# member-as-attribute (xmlAttribute)
auto o = xml.element("Object")
o.set_attr("key", "photo.jpg")           # write; read with o.get_attr("key")

# element renaming (locationName): just choose the tag name
auto e = xml.element("WhateverTheModelSays")

# wrapped list: <Items><Item/>..</Items>
auto items = wrapper.find("Items").unwrap().find_all("Item")

# flattened list: repeated <Item/> siblings, no wrapper
auto flat = parent.find_all("Item")

# namespaces: a declaration is just an attribute; a prefixed name just has a colon
root.set_attr("xmlns", "http://s3.amazonaws.com/doc/2006-03-01/")
auto pfx = xml.element("ns:Thing")       # pfx.local_name() == "Thing"
```

### Navigating AWS error documents

Both error shapes are plain navigation. The query-protocol wrapper:

```zap
auto r = xml.parse(body)                 # <ErrorResponse xmlns="…"><Error>…
auto err = r.find("Error").unwrap()
print(err.namespace)                     # inherited namespace URI
auto code = err.find("Code").unwrap().get_text()
auto msg = err.find("Message").unwrap().get_text()
```

The rest-xml bare variant, where the root *is* the error:

```zap
auto r = xml.parse(body)                 # <Error><Code>…</Code>…</Error>
auto code = r.find("Code").unwrap().get_text()
```

## Limitations (v1)

- Mixed content order (text interleaved between child elements) is not preserved.
- A DOCTYPE internal subset (`<!DOCTYPE x [ … ]>`) is skipped naively to the next
  `>` (AWS never emits one).
- No streaming pull-parser yet — a DOM parse holds the whole document. A
  streaming reader for very large S3 listings is a planned follow-up.
