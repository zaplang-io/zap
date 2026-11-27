# Golden fixture: in-place replacement of a resolved Namespace binding (#1698).
# A parameter-validation / transform callback (zaplang-io/cli#14, via the #1544
# handler adapter) runs after type conversion and must write its typed result
# back into the parsed Namespace. This exercises the typed setters (`set_str`,
# `set_int`, `set_float`, `set_flag`) and the universal raw `set`, and pins the
# invariants: argv presence (`has`) and every other binding survive a
# replacement, a separate parse is untouched, a binding that reached the
# namespace through a parent parser selecting a subcommand can be replaced, and
# an unknown destination raises a catchable `KeyError` — the documented typed
# error path.
def supplied_string():
    # A value supplied on argv: replacing it keeps `has` True.
    auto p = argparse.ArgumentParser()
    p.add_argument("--name", default="anon")
    auto ns = p.parse_args(["--name", "zap"])
    print(ns.get_str("name"))
    print(ns.has("name"))
    ns.set_str("name", "rust")
    print(ns.get_str("name"))
    print(ns.has("name"))

def default_int():
    # An int that reached the namespace through the parser's `default` (the
    # environment-sourced-default path): the replacement is seen by `get_int`
    # and the binding stays marked not-present.
    auto p = argparse.ArgumentParser()
    p.add_argument("--jobs", type="int", default="2")
    auto ns = p.parse_args([])
    print(ns.get_int("jobs"))
    print(ns.has("jobs"))
    ns.set_int("jobs", 4)
    print(ns.get_int("jobs"))
    print(ns.has("jobs"))

def declared_bool_default():
    # A declared boolean default, flipped both ways; `has` stays False.
    auto p = argparse.ArgumentParser()
    p.add_argument("--verbose", action="store_true")
    auto ns = p.parse_args([])
    print(ns.get_flag("verbose"))
    print(ns.has("verbose"))
    ns.set_flag("verbose", True)
    print(ns.get_flag("verbose"))
    print(ns.has("verbose"))
    ns.set_flag("verbose", False)
    print(ns.get_flag("verbose"))

def raw_and_float():
    # The universal string setter (`set_str`) and the float setter.
    auto p = argparse.ArgumentParser()
    p.add_argument("--name", default="x")
    p.add_argument("--ratio", type="float", default="0.5")
    auto ns = p.parse_args(["--name", "y", "--ratio", "1.5"])
    ns.set_str("name", "z")
    ns.set_float("ratio", 2.5)
    print(ns.get_str("name"))
    print(ns.get_float("ratio"))

def others_unaffected():
    # Replacing one binding leaves every sibling binding and a separate parse
    # of the same parser untouched.
    auto p = argparse.ArgumentParser()
    p.add_argument("--a", type="int", default="1")
    p.add_argument("--b", type="int", default="2")
    auto ns1 = p.parse_args(["--a", "10", "--b", "20"])
    auto ns2 = p.parse_args(["--a", "100", "--b", "200"])
    ns1.set_int("a", 999)
    print(ns1.get_int("a"))
    print(ns1.get_int("b"))
    print(ns2.get_int("a"))
    print(ns2.get_int("b"))

def subcommand_binding():
    # A binding that reached the namespace through a parent parser selecting a
    # subcommand (the child's positional / flag) can be replaced too.
    auto p = argparse.ArgumentParser(prog="git")
    auto subs = p.add_subparsers(dest="cmd")
    auto build = subs.add_parser("build")
    build.add_argument("--release", action="store_true")
    build.add_argument("target")
    auto ns = p.parse_args(["build", "--release", "app"])
    print(ns.get("target"))
    print(ns.has("target"))
    ns.set_str("target", "lib")
    ns.set_flag("release", False)
    print(ns.get("target"))
    print(ns.has("target"))
    print(ns.get_flag("release"))

def unknown_generic():
    # An unknown destination is a catchable error (caught generically here).
    auto p = argparse.ArgumentParser()
    p.add_argument("--x", type="int", default="0")
    auto ns = p.parse_args([])
    try:
        ns.set_int("nope", 1)
        print("no-error")
    except Exception:
        print("unknown-caught")

def unknown_typed():
    # The documented typed error path: a nonexistent destination raises
    # `KeyError` specifically, leaving the existing bindings intact.
    auto p = argparse.ArgumentParser()
    p.add_argument("--x", default="0")
    auto ns = p.parse_args([])
    try:
        ns.set_str("ghost", "v")
        print("no-error")
    except KeyError:
        print("keyerror-caught")
    print(ns.get_str("x"))

def main():
    supplied_string()
    default_int()
    declared_bool_default()
    raw_and_float()
    others_unaffected()
    subcommand_binding()
    unknown_generic()
    unknown_typed()
