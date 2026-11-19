# Golden fixture: defaults, `has`, typed conversion and valid `choices` (#997).
# With an empty argv every declared argument takes its default; a second parse
# supplies values (including a valid choice) and overrides them.
def main():
    auto p = argparse.ArgumentParser()
    p.add_argument("--name", default="anon")
    p.add_argument("--count", type="int", default="5")
    p.add_argument("--verbose", action="store_true")
    p.add_argument("--mode", choices=["fast", "slow"], default="fast")
    auto ns = p.parse_args([])
    print(ns.get("name"))
    print(ns.get_int("count"))
    print(ns.get_flag("verbose"))
    print(ns.get("mode"))
    print(ns.has("name"))
    auto ns2 = p.parse_args(["--name", "zap", "--mode", "slow", "--verbose"])
    print(ns2.get("name"))
    print(ns2.get("mode"))
    print(ns2.get_flag("verbose"))
    print(ns2.has("name"))
