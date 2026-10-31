# First-class function values with struct parameter/return types, invoked
# indirectly through a `fn(Req) -> Resp` parameter (I#465, D-0067). This is the
# shape that exposed the soundness hole: a named handler coerced to a `fn(...)`
# value and called through the expected signature. With incompatible coercions
# now rejected in lowering, the `<fn>$fnval` thunk and the call site always
# agree on arity / parameter types / return type, so the legitimate case must
# still compile and run correctly end-to-end.
struct Req:
    str path

struct Resp:
    int64 status

def ok(Req r) -> Resp:
    return Resp(200)

def not_found(Req r) -> Resp:
    return Resp(404)

# Dispatch through a function-typed parameter: the handler is passed as a value
# and invoked indirectly, exercising the closure thunk + CallClosure path.
def serve(fn(Req) -> Resp handler, Req r) -> Resp:
    return handler(r)

def main():
    Req r = Req("/health")
    print(serve(ok, r).status)
    print(serve(not_found, r).status)
    # A lambda handler coerces to the same `fn(Req) -> Resp` type.
    print(serve(lambda req: Resp(201), r).status)
