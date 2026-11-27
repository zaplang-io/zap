# Golden fixture: the synthesized handler reaches its parameter past optional
# named metadata on the decorator factory (#1626).
#
# `command(str name="", fn(Namespace) handler)` declares optional `name=`
# metadata before the handler parameter. The adapter is passed to `handler` BY
# NAME, so `name=` binds its default (or a supplied keyword) rather than being
# filled by the handler positionally
# (decisions/lang/1626-adapter-metadata-binding.md):
#
#     @cli.command(name="alias")   ==>   greet = cli.command(name="alias",
#     def greet(...)                                          handler=greet$adapter)
#     @cli.command                 ==>   plain = cli.command(handler=plain$adapter,
#     def plain(...)                                          name="plain")
#
# An omitted `name=` now defaults to the decorated declaration's name (#1665), so
# `plain`'s command is labelled "plain" (the second output line).

struct Command:
    str label
    fn(Namespace) -> str run_fn

    def run(self, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("who")
        auto ns = p.parse_args(argv)
        fn(Namespace) -> str f = self.run_fn
        return self.label + ":" + f(ns)

struct Cli:
    def command(self, str name = "", fn(Namespace) -> str handler) -> Command:
        return Command(label=name, run_fn=handler)

@cli.command(name="alias")
def greet(str who) -> str:
    return who

@cli.command
def plain(str who) -> str:
    return who

def main():
    auto cli = Cli()
    print(greet.run(["Ada"]))
    print(plain.run(["Bo"]))
