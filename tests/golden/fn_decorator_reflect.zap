# Golden fixture: declared decorators/annotations on a FUNCTION (I#457, D-0087).
#
# #26/D-0036 shipped struct/field/enum reflection but left function decorators
# erroring ZAP6001 ("not implemented yet"). A bare, declared `annotation` or
# `decorator` on a `def` now attaches reflection metadata keyed by the function
# name — the same model as struct-level reflection — queryable with `reflect_has`
# / `reflect_names`. The function body lowers unchanged; the decorator is the
# metadata (behavioral wrap/before/after codegen stays the deferred user-macro
# piece). Decorators stack, recorded in source order.

annotation Route:
    str path = ""

decorator audited:
    pass

@Route(path="/users")
@audited
def list_users() -> int64:
    return 0

def main():
    if reflect_has("list_users", "Route"):
        print("list_users @Route")
    if reflect_has("list_users", "audited"):
        print("list_users @audited")
    if not reflect_has("list_users", "Nope"):
        print("no @Nope")
    print(reflect_names("list_users"))
