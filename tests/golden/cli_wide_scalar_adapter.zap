# Golden fixture: the handler adapter binds wider scalar parameter types (#1625).
#
# The argparse `Namespace` exposes `get_int` (int64) and `get_float` (float64);
# a narrower scalar reads the wide getter and converts, e.g. `int32 count` ->
# `int32(ns.get_int("count"))` and `uint8 level` -> `uint8(ns.get_int("level"))`
# (decisions/lang/1625-adapter-wide-scalars.md).

struct Command:
    fn(Namespace) -> str run_fn

    def run(self, list[str] argv) -> str:
        auto p = argparse.ArgumentParser()
        p.add_argument("--count", type="int", default="1")
        p.add_argument("--level", type="int", default="0")
        auto ns = p.parse_args(argv)
        fn(Namespace) -> str f = self.run_fn
        return f(ns)

struct Cli:
    def command(self, fn(Namespace) -> str handler) -> Command:
        return Command(run_fn=handler)

@cli.command
def greet(int32 count, uint8 level) -> str:
    return str(count) + "/" + str(level)

def main():
    auto cli = Cli()
    print(greet.run(["--count", "7", "--level", "200"]))
