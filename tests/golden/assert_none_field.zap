# Golden (#1184): `assert` on a nullable reference struct field compared with
# `None` compiles and runs — both while the field holds its default `None` and
# after it is assigned a struct. Previously `assert field == None` failed in
# lowering (`ZAP6001`, "Phase 1 slice") because the power-assert decomposition
# (D-0054) evaluated the bare `None` operand, which has no standalone lowering.
# `== None` / `!= None` now take the same null check as `is None` / `is not
# None` (a real `IsNull` test) and are no longer decomposed.


struct Reader:
    bytes pending


struct Client:
    Reader reader = None


def main():
    auto client = Client()
    # Empty field — the default `None`.
    assert client.reader == None
    assert client.reader is None
    print("empty: reader is None")

    # After assigning a struct, the field is no longer `None`.
    client.reader = Reader(pending=b"hello")
    assert client.reader != None
    assert client.reader is not None
    print("assigned: reader is not None")

    print("done")
