# Golden fixture: positional decorator metadata around a NON-LAST handler
# parameter (#1645).
#
# `argument(str name, fn(Namespace) handler, str metavar = "")` declares the
# handler at index 1 (not last). A decorator with two POSITIONAL metadata
# arguments must route the second positional PAST the handler's slot to
# `metavar` by name — it must not land on (or double-bind) the handler, which
# the synthesized adapter fills by keyword
# (decisions/lang/1645-adapter-positional-metadata.md):
#
#     @cli.argument("pos-name", "META")   ==>   greet = cli.argument(
#     def greet(...)                                "pos-name", metavar="META",
#                                                   handler=greet$adapter)

struct Arg:
    str name
    str metavar
    fn(Namespace) -> str run_fn

    def run(self, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("who")
        auto ns = p.parse_args(argv)
        fn(Namespace) -> str f = self.run_fn
        return self.name + "/" + self.metavar + ":" + f(ns)

struct Cli:
    def argument(self, str name, fn(Namespace) -> str handler, str metavar = "") -> Arg:
        return Arg(name=name, metavar=metavar, run_fn=handler)

@cli.argument("pos-name", "META")
def greet(str who) -> str:
    return who

def main():
    auto cli = Cli()
    print(greet.run(["Ada"]))
