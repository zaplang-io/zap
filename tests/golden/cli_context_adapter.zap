# Golden fixture: an explicit typed context through the argparse-backed handler
# adapter (#1621, cli#7).
#
# A handler may declare a context struct as its FIRST parameter; the dispatcher
# supplies it directly. The receiver method opts in with the seam
# `fn(Context, Namespace) -> R` (one leading non-Namespace parameter + the
# trailing Namespace). The synthesized adapter takes `(Context ctx, Namespace
# ns)`, passes `ctx` straight to the handler's context parameter, and reads the
# remaining scalars from the namespace by name — in the handler's signature
# order (decisions/lang/1621-context-adapter.md).
#
#     @cli.command                      def greet$adapter(Context ctx, Namespace ns) -> str:
#     def greet(Context ctx,                return greet$impl(ctx,
#               int64 count,                                  ns.get_int("count"),
#               str name) -> str:                             ns.get_str("name"))

struct Context:
    str command_name
    list[str] command_path

struct Command:
    fn(Context, Namespace) -> str run_fn

    def run(self, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("--count", type="int", default="1")
        p.add_argument("name")
        auto ns = p.parse_args(argv)
        Context ctx = Context(command_name="greet", command_path=["greet"])
        fn(Context, Namespace) -> str f = self.run_fn
        return f(ctx, ns)

struct Cli:
    def command(self, fn(Context, Namespace) -> str handler) -> Command:
        return Command(run_fn=handler)

@cli.command
def greet(Context ctx, int64 count, str name) -> str:
    return ctx.command_name + ":" + name + " x" + str(count)

def main():
    auto cli = Cli()
    print(greet.run(["--count", "2", "Ada"]))
    print(greet.run(["Bo"]))
