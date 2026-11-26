# Golden fixture: an injected `Depends(provider)` parameter composes with the
# #1621 context and the namespace scalars in an argparse-backed handler (#1646).
#
# The adapter resolves the provider into a `<provider>$dep` local (deps-first,
# once each — the D-0092 resolver, reused) and passes it to the handler; the
# context flows from the dispatcher, the scalar from the Namespace. A `Depends`
# parameter does not widen the `fn(Context, Namespace)` seam
# (decisions/lang/1646-adapter-injected-depends.md):
#
#     @cli.command                                   ==>   greet$impl($ctx,
#     def greet(Context ctx, DB db = Depends(get_db),              get_db$dep,
#               str who)                                           $ns.get_str("who"))

struct Context:
    str name

struct DB:
    str dsn

def get_db() -> DB:
    return DB(dsn="db://x")

struct Command:
    fn(Context, Namespace) -> str run_fn

    def run(self, Context c, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("who")
        auto ns = p.parse_args(argv)
        fn(Context, Namespace) -> str f = self.run_fn
        return f(c, ns)

struct Cli:
    def command(self, fn(Context, Namespace) -> str handler) -> Command:
        return Command(run_fn=handler)

@cli.command
def greet(Context ctx, DB db = Depends(get_db), str who) -> str:
    return ctx.name + "/" + db.dsn + "/" + who

def main():
    auto cli = Cli()
    auto c = Context(name="cli")
    print(greet.run(c, ["Ada"]))
