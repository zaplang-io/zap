# Golden fixture: non-numeric scalar handler parameters bound from the command
# line (#1655).
#
# A handler declaring `char` / `bytes` parameters binds them from the argparse
# `Namespace` through the dedicated typed getters `get_char` / `get_bytes`
# (decisions/lang/1655-namespace-nonnumeric-getters.md):
#
#     @cli.command                 ==>   show$impl(ns.get_char("c"),
#     def show(char c, bytes b)                     ns.get_bytes("b"))
#
# `get_char` requires exactly one Unicode codepoint; `get_bytes` is the argument
# string's UTF-8 encoding (so a 2-byte `é` makes "héllo" six bytes).

struct Command:
    fn(Namespace) -> str run_fn

    def run(self, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("c")
        p.add_argument("b")
        auto ns = p.parse_args(argv)
        fn(Namespace) -> str f = self.run_fn
        return f(ns)

struct Cli:
    def command(self, fn(Namespace) -> str handler) -> Command:
        return Command(run_fn=handler)

@cli.command
def show(char c, bytes b) -> str:
    return str(c) + "/" + str(len(b))

def main():
    auto cli = Cli()
    print(show.run(["Z", "héllo"]))
    # `get_char` requires exactly one codepoint: a multi-character argument raises
    # a catchable `ValueError` from inside the synthesized adapter.
    try:
        print(show.run(["ab", "x"]))
    except ValueError:
        print("bad-char")
