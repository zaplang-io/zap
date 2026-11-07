# Golden (epic #374, B3): `== None` / `!= None` behave like `is None` /
# `is not None` on an Optional (SPEC §5). Previously they passed sema but failed
# in lowering (ZAP6001); now they are the same null/empty check.
def main():
    Optional[int64] a = None
    Optional[int64] b = Optional.Some(5)
    print(a == None)
    print(a != None)
    print(b == None)
    print(b != None)
