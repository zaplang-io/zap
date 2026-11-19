# Golden fixture: argument groups + mutually-exclusive groups (#997). Group
# members parse exactly like top-level arguments; a mutually-exclusive group
# rejects two of its members appearing together (a catchable error).
def main():
    auto p = argparse.ArgumentParser(prog="tool")
    auto g = p.add_argument_group("input options")
    g.add_argument("--in-file", default="-")
    g.add_argument("--encoding", default="utf-8")
    auto mx = p.add_mutually_exclusive_group()
    mx.add_argument("--json", action="store_true")
    mx.add_argument("--yaml", action="store_true")
    auto ns = p.parse_args(["--in-file", "data.txt", "--json"])
    print(ns.get("in_file"))
    print(ns.get("encoding"))
    print(ns.get_flag("json"))
    print(ns.get_flag("yaml"))
    try:
        p.parse_args(["--json", "--yaml"])
        print("no-error")
    except Exception:
        print("mx-conflict-caught")
