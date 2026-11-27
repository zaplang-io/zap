# Golden fixture: an omitted `name=` defaults to the decorated declaration's name
# (#1665, Click-style). `cli.command(str name = "", ...)` with a bare `@cli.command`
# receives `name="greet"` from the desugar
# (decisions/lang/1665-decl-name-default.md):
#
#     @cli.command          ==>   greet = cli.command(handler=greet$adapter,
#     def greet(...)                                   name="greet")

struct Command:
    str name
    fn(Namespace) -> str run_fn

    def run(self, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("who")
        auto ns = p.parse_args(argv)
        fn(Namespace) -> str f = self.run_fn
        return self.name + ":" + f(ns)

struct Cli:
    def command(self, str name = "", fn(Namespace) -> str handler) -> Command:
        return Command(name=name, run_fn=handler)

@cli.command
def greet(str who) -> str:
    return who

def main():
    auto cli = Cli()
    print(greet.name)
    print(greet.run(["Ada"]))
