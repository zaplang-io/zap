# Golden fixture (#1668): a generic returning decorator-style factory dispatches
# on its own type parameter with a `match`. For `Inner = Command` the nominal
# struct arm is irrefutable, so the callable `case _:` arm — which treats the
# capture as a `fn(..)` — is statically unreachable and must be elided before
# lowering (before the fix it failed with ZAP4001, "value of type `Command` is
# not a function"). For `Inner = fn(..)` the struct arm is statically dead
# (#1326) and the `_` arm builds the command. One generic body serves both the
# innermost (callable) and outer (composed) stack positions.

struct Context:
    str name

struct Command:
    str name
    fn(Context, Namespace) handler

def build[Inner](Inner previous, str label) -> Command:
    match previous:
        case Command() as composed:
            return composed
        case _:
            fn(Context, Namespace) handler = previous
            return Command(name=label, handler=handler)

def prepare[Inner](str label, Inner previous) -> Command:
    return build(previous, label)

def callback(Context ctx, Namespace ns):
    print(ctx.name)

def main():
    fn(Context, Namespace) handler = callback
    auto first = prepare("inner", handler)
    auto second = prepare("outer", first)
    print(second.name)
