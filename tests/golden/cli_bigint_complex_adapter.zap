# Golden fixture: bigint / complex handler parameters bound from the command line
# (#1657 getters, finishing #1655's deferred scalars).
#
# The adapter binds `bigint`/`complex` through the dedicated typed getters
# `get_bigint`/`get_complex`, which parse the argument string
# (decisions/lang/1657-str-parse-conversions.md):
#
#     @cli.command                       ==>   show$impl(ns.get_bigint("n"),
#     def show(bigint n, complex z)                       ns.get_complex("z"))

struct Command:
    fn(Namespace) -> str run_fn

    def run(self, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("n")
        p.add_argument("z")
        auto ns = p.parse_args(argv)
        fn(Namespace) -> str f = self.run_fn
        return f(ns)

struct Cli:
    def command(self, fn(Namespace) -> str handler) -> Command:
        return Command(run_fn=handler)

@cli.command
def show(bigint n, complex z) -> str:
    return str(n) + "|" + str(z)

def main():
    auto cli = Cli()
    print(show.run(["100000000000000000000", "1+2j"]))
