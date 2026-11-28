# Golden fixture (#1713): CPython-parity PARSING behaviors of the argparse
# object parser — parse_known_args, short-option grouping and attached values,
# unambiguous long-option abbreviation, the `--` end-of-options separator with a
# nargs-`*` positional, `add_help=False`, intermixed parsing, and the negative
# non-integer value fix (B1). Values are printed (never raw lists) so the golden
# text is exact.
def known_args():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--jobs", type="int")
    auto r = p.parse_known_args(["--jobs", "2", "x", "--bad", "y"])
    print(r[0].get_int("jobs"))
    print(len(r[1]))
    for e in r[1]:
        print(e)


def short_grouping():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("-x", action="store_true")
    p.add_argument("-v", action="store_true")
    p.add_argument("-f", action="store_true")
    p.add_argument("-o")
    auto ns = p.parse_args(["-xvf", "-oout.txt"])
    print(ns.get_flag("x"))
    print(ns.get_flag("v"))
    print(ns.get_flag("f"))
    print(ns.get("o"))


def short_equals_and_spaced():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("-o")
    print(p.parse_args(["-o=val"]).get("o"))
    print(p.parse_args(["-o", "spaced"]).get("o"))


def abbrev():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--verbose", action="store_true")
    p.add_argument("--version", action="store_true")
    p.add_argument("--count", type="int")
    auto ns = p.parse_args(["--verb", "--co", "3"])
    print(ns.get_flag("verbose"))
    print(ns.get_int("count"))


def negnum():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--temp", type="float")
    p.add_argument("--n", type="int")
    auto ns = p.parse_args(["--temp", "-1.5", "--n", "-3"])
    print(ns.get("temp"))
    print(ns.get_int("n"))


def end_of_opts():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--flag", action="store_true")
    p.add_argument("rest", nargs="*")
    auto ns = p.parse_args(["--flag", "--", "--notflag", "-x"])
    print(ns.get_flag("flag"))
    auto r = ns.get_list("rest")
    print(len(r))
    for e in r:
        print(e)


def no_add_help():
    auto p = argparse.ArgumentParser(prog="p")
    p.set_add_help(False)
    p.add_argument("-h", action="store_true", help="height")
    print(p.parse_args(["-h"]).get_flag("h"))
    print(p.format_usage())


def intermixed():
    auto p = argparse.ArgumentParser(prog="p")
    p.add_argument("--flag", action="store_true")
    p.add_argument("a")
    p.add_argument("b")
    auto ns = p.parse_intermixed_args(["x", "--flag", "y"])
    print(ns.get("a"))
    print(ns.get("b"))
    print(ns.get_flag("flag"))


def main():
    known_args()
    print("---")
    short_grouping()
    print("---")
    short_equals_and_spaced()
    print("---")
    abbrev()
    print("---")
    negnum()
    print("---")
    end_of_opts()
    print("---")
    no_add_help()
    print("---")
    intermixed()
