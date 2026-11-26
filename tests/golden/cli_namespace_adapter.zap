# Golden fixture: an argparse-backed handler adapter (#1544).
#
# A returning decorator whose receiver method declares its handler parameter as
# the uniform argparse seam `fn(Namespace) -> R` receives a compiler-synthesized
# adapter, not the raw handler. The adapter reads each of the decorated handler's
# parameters from the parsed `Namespace` BY NAME with the type-matched getter
# (int -> get_int, bool -> get_flag, str -> get_str) and calls the handler in its
# declaration order. So one uniform `fn(Namespace) -> R` dispatches a handler of
# ANY signature — the capability #1544 adds (a fixed `fn(...)` handled only one
# shape). Adaptation is static: no runtime invoke-by-name
# (decisions/lang/1544-handler-adapter-synthesis.md, mechanism A-argparse).
#
#     @cli.command                 greet = cli.command(greet$adapter)   # not $impl
#     def greet(int64 count,       def greet$adapter(Namespace ns) -> str:
#               bool shout,            return greet$impl(ns.get_int("count"),
#               str name) -> str:                        ns.get_flag("shout"),
#         ...                                             ns.get_str("name"))

struct Command:
    fn(Namespace) -> str run_fn

    def run(self, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("--count", type="int", default="1")
        p.add_argument("--shout", action="store_true")
        p.add_argument("name")
        auto ns = p.parse_args(argv)
        fn(Namespace) -> str f = self.run_fn
        return f(ns)

struct Cli:
    def command(self, fn(Namespace) -> str handler) -> Command:
        return Command(run_fn=handler)

@cli.command
def greet(int64 count, bool shout, str name) -> str:
    str tag = "?"
    if shout:
        tag = "!"
    return name + tag + " x" + str(count)

def main():
    auto cli = Cli()
    # `greet` is now the Command. Dispatch twice with different argv to prove the
    # adapter binds each parameter by name and type from the parsed namespace.
    print(greet.run(["--count", "3", "--shout", "Ada"]))
    print(greet.run(["Bo"]))
