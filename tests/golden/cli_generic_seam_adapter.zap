# Golden fixture: a uniform generic seam (#1647). A receiver method whose handler
# parameter is typed as its own generic (`Inner previous`) is recognized as the
# handler seam, so the innermost use adapts the decorated handler into it — with
# the context inferred from the handler's first non-scalar parameter and scalars
# bound from the Namespace (decisions/lang/1647-uniform-generic-seam.md):
#
#     @cli.command                        ==>   greet = cli.command(
#     def greet(Context ctx, str who)                 previous=greet$adapter)
#
# where greet$adapter is fn(Context, Namespace) -> str (context inferred).

struct Context:
    str name

struct Command:
    fn(Context, Namespace) -> str run_fn

    def run(self, Context c, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("who")
        auto ns = p.parse_args(argv)
        fn(Context, Namespace) -> str f = self.run_fn
        return c.name + ":" + f(c, ns)

struct Cli:
    def command[Inner](self, Inner previous) -> Command:
        return Command(run_fn=previous)

@cli.command
def greet(Context ctx, str who) -> str:
    return who

def main():
    auto cli = Cli()
    auto c = Context(name="cmd")
    print(greet.run(c, ["Ada"]))
