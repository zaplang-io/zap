# Golden fixture: options hidden from usage/help (#1702). A `hidden=True`
# argument registers and parses EXACTLY like a visible one — defaults, typed
# values, explicit presence, required enforcement, and mutual exclusion are all
# unchanged — while `format_help()` omits it from the usage line and every help
# row. Visibility is display-only; the walled garden here is that nothing about
# parsing shifts when an argument is marked hidden.
def hidden_flag_parses():
    auto p = argparse.ArgumentParser(prog="demo")
    p.add_argument("--visible", action="store_true", help="Public flag")
    p.add_argument(
        "--internal",
        action="store_true",
        hidden=True,
        help="Compatibility flag",
    )
    auto supplied = p.parse_args(["--internal"])
    print(supplied.get_flag("internal"))
    print(supplied.has("internal"))
    print(supplied.get_flag("visible"))
    print(supplied.has("visible"))


def hidden_absent_from_help():
    auto p = argparse.ArgumentParser(prog="demo")
    p.add_argument("--visible", action="store_true", help="Public flag")
    p.add_argument(
        "--internal",
        action="store_true",
        hidden=True,
        help="Compatibility flag",
    )
    auto h = p.format_help()
    print(h.find("--visible") >= 0)
    print(h.find("--internal") < 0)
    print(h.find("Compatibility flag") < 0)
    print(h.find("Public flag") >= 0)


def hidden_value_defaults_and_typing():
    # A hidden value option keeps its default when absent and validates its type
    # when supplied, exactly like a visible one.
    auto p = argparse.ArgumentParser(prog="demo")
    p.add_argument("--count", type="int", default="7", hidden=True)
    auto absent = p.parse_args([])
    print(absent.get_int("count"))
    print(absent.has("count"))
    auto given = p.parse_args(["--count", "42"])
    print(given.get_int("count"))
    print(given.has("count"))


def hidden_bad_value_still_diagnosed():
    # An explicitly supplied malformed hidden option still raises a catchable
    # error — hidden suppresses display, never validation.
    auto p = argparse.ArgumentParser(prog="demo")
    p.add_argument("--count", type="int", hidden=True)
    try:
        p.parse_args(["--count", "nope"])
        print("no-error")
    except Exception:
        print("bad-value-caught")


def hidden_required_still_enforced():
    # A hidden required option must still appear.
    auto p = argparse.ArgumentParser(prog="demo")
    p.add_argument("--token", required=True, hidden=True)
    try:
        p.parse_args([])
        print("no-error")
    except Exception:
        print("missing-required-caught")
    auto ok = p.parse_args(["--token", "abc"])
    print(ok.get("token"))


def hidden_mutual_exclusion_still_enforced():
    # Hidden members of a mutually-exclusive group still conflict when two are
    # supplied together.
    auto p = argparse.ArgumentParser(prog="demo")
    auto mx = p.add_mutually_exclusive_group()
    mx.add_argument("--json", action="store_true", hidden=True)
    mx.add_argument("--yaml", action="store_true", hidden=True)
    auto one = p.parse_args(["--json"])
    print(one.get_flag("json"))
    try:
        p.parse_args(["--json", "--yaml"])
        print("no-error")
    except Exception:
        print("mx-conflict-caught")


def main():
    hidden_flag_parses()
    hidden_absent_from_help()
    hidden_value_defaults_and_typing()
    hidden_bad_value_still_diagnosed()
    hidden_required_still_enforced()
    hidden_mutual_exclusion_still_enforced()
