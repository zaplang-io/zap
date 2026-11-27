# Golden fixture (#1669): a generic FREE function infers its type parameter from
# a bare callable reference — a named function passed directly, with no typed
# local binding to carry the signature. `wrap(greet)` pins
# `Inner = fn(Context, str) -> str` from the function value's own signature,
# returns it, and the result is called at its exact shape. Before the fix the
# monomorphizer saw the bare name as an untyped value and failed with ZAP6001
# ("cannot infer type argument 'Inner'").

struct Context:
    str name

def wrap[Inner](Inner previous) -> Inner:
    return previous

def greet(Context ctx, str who) -> str:
    return ctx.name + ":" + who

def main():
    auto g = wrap(greet)
    print(g(Context(name="cmd"), "Ada"))
