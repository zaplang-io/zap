# Golden fixture: the AWS shape features the xml module must EXPRESS (I#351,
# D-0083). Wrapped vs flattened lists, member-as-attribute (xmlAttribute),
# element renaming (locationName), and navigating a real AWS-style error document
# (both the <ErrorResponse> wrapper and the bare <Error> variants).
def main():
    # Wrapped list: <Items><Item>..</Item>..</Items> — navigate wrapper then
    # find_all the members.
    auto wrapped = "<Bucket><Items><Item>a</Item><Item>b</Item></Items></Bucket>"
    auto w = xml.parse(wrapped)
    auto wi = w.find("Items").unwrap().find_all("Item")
    print(len(wi))  # 2
    print(wi[0].get_text())  # a
    print(wi[1].get_text())  # b

    # Flattened list: repeated <Item> siblings with no wrapper — find_all on the
    # parent returns every one.
    auto flat = "<Bucket><Item>x</Item><Item>y</Item><Item>z</Item></Bucket>"
    auto f = xml.parse(flat)
    auto fi = f.find_all("Item")
    print(len(fi))  # 3
    for it in fi:
        print(it.get_text())  # x, y, z

    # member-as-attribute (xmlAttribute): a value carried as an attribute, read
    # with get_attr and written with set_attr.
    auto obj = xml.element("Object")
    obj.set_attr("key", "photo.jpg")
    obj.set_text("")
    auto objs = xml.dumps(obj)
    print(objs)  # <Object key="photo.jpg"/>
    print(xml.parse(objs).get_attr("key").unwrap())  # photo.jpg

    # Element renaming (locationName): the caller controls the tag name.
    auto renamed = xml.element("CreateBucketConfiguration")
    auto loc = xml.element("LocationConstraint")
    loc.set_text("us-west-2")
    renamed.add_child(loc)
    print(xml.dumps(renamed))  # <CreateBucketConfiguration><LocationConstraint>us-west-2</LocationConstraint></CreateBucketConfiguration>

    # AWS query-protocol error: <ErrorResponse><Error><Code>/<Message>, with the
    # namespace inherited by the nested <Error>.
    auto errdoc = "<ErrorResponse xmlns=\"urn:aws\"><Error><Code>AccessDenied</Code><Message>denied</Message></Error><RequestId>abc</RequestId></ErrorResponse>"
    auto er = xml.parse(errdoc)
    auto err = er.find("Error").unwrap()
    print(err.namespace)  # urn:aws (inherited)
    print(err.find("Code").unwrap().get_text())  # AccessDenied
    print(err.find("Message").unwrap().get_text())  # denied
    print(er.find("RequestId").unwrap().get_text())  # abc

    # rest-xml bare <Error> variant (root is already the Error element).
    auto bare = "<Error><Code>NoSuchKey</Code><Message>missing</Message></Error>"
    auto be = xml.parse(bare)
    print(be.find("Code").unwrap().get_text())  # NoSuchKey
    print(be.find("Message").unwrap().get_text())  # missing
