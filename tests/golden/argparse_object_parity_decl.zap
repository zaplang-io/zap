# Golden fixture (#1713): CPython-parity DECLARATION behaviors of the argparse
# object parser — the extended action set (store_const, count, append,
# append_const, version), nargs value forms (N, *, ?+const), the extended type
# tags (bigint) with choices compared against the converted value (G10),
# `dest=` override, parser-wide argument_default, and set_defaults/get_default.
def store_const_count_append():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--mode", action="store_const", const_value="fast")
    p.add_argument("--vv", action="count")
    p.add_argument("--item", action="append")
    p.add_argument("--flags", action="append_const", const_value="X")
    auto ns = p.parse_args(
        [
            "--mode",
            "--vv",
            "--vv",
            "--vv",
            "--item",
            "a",
            "--item",
            "b",
            "--flags",
            "--flags",
        ],
    )
    print(ns.get("mode"))
    print(ns.get_count("vv"))
    auto it = ns.get_list("item")
    print(len(it))
    for e in it:
        print(e)
    print(len(ns.get_list("flags")))


def nargs_values():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--coords", type="int", nargs="2")
    p.add_argument("--extra", type="int", nargs="*")
    p.add_argument("--maybe", nargs="?", const_value="C", default="D")
    auto ns = p.parse_args(["--coords", "3", "4", "--extra", "5", "6", "7", "--maybe"])
    auto c = ns.get_int_list("coords")
    print(len(c))
    for v in c:
        print(v)
    print(len(ns.get_list("extra")))
    print(ns.get("maybe"))


def type_and_choices():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--level", type="int", choices=["1", "2", "3"])
    p.add_argument("--big", type="bigint")
    auto ns = p.parse_args(["--level", "02", "--big", "123456789012345678901234567890"])
    print(ns.get_int("level"))
    print(ns.get("big"))


def dest_override_and_defaults():
    auto p = argparse.ArgumentParser(prog="p")
    p.set_argument_default("Z")
    p.add_argument("--output", dest="out")
    p.add_argument("--plain")
    p.set_default("extra", "seeded")
    auto ns = p.parse_args(["--output", "file"])
    print(ns.get("out"))
    print(ns.get("plain"))
    print(ns.get("extra"))
    print(p.get_default("plain"))


def version_action():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--version", action="version", version="p 1.2.3")
    auto ns = p.parse_args(["--version"])
    print(ns.version_requested())
    print(ns.get("version"))
    print(p.parse_args([]).version_requested())


def main():
    store_const_count_append()
    print("---")
    nargs_values()
    print("---")
    type_and_choices()
    print("---")
    dest_override_and_defaults()
    print("---")
    version_action()
