# Golden fixture: explicit boolean defaults for store_true/store_false flags
# (#1692). An omitted `default` keeps the action's implied value (False for
# store_true, True for store_false); an explicit "true"/"false" seeds the absent
# flag verbatim. Supplying a flag always applies its action, consumes no value,
# and marks the destination present; an absent flag with an explicit default is
# NOT present. An invalid boolean default raises a catchable error at declaration.
def action_defaults_when_omitted():
    auto p = argparse.ArgumentParser()
    p.add_argument("--enabled", action="store_true")
    p.add_argument("--disabled", action="store_false")
    auto absent = p.parse_args([])
    print(absent.get_flag("enabled"))
    print(absent.get_flag("disabled"))
    print(absent.has("enabled"))
    print(absent.has("disabled"))


def explicit_defaults_preserved():
    auto p = argparse.ArgumentParser()
    p.add_argument("--enabled", action="store_true", default="true")
    p.add_argument("--disabled", action="store_false", default="false")
    auto absent = p.parse_args([])
    # Explicit defaults seed the absent value...
    print(absent.get_flag("enabled"))
    print(absent.get_flag("disabled"))
    # ...without marking the flag argv-present.
    print(absent.has("enabled"))
    print(absent.has("disabled"))


def explicit_default_matching_action():
    # store_true default="false" and store_false default="true" restate the
    # action default explicitly; behaviour is identical to omitting it.
    auto p = argparse.ArgumentParser()
    p.add_argument("--enabled", action="store_true", default="false")
    p.add_argument("--disabled", action="store_false", default="true")
    auto absent = p.parse_args([])
    print(absent.get_flag("enabled"))
    print(absent.get_flag("disabled"))


def supplying_overrides_default():
    # Supplying a flag applies its action regardless of the default, consumes no
    # value, and marks it present.
    auto p = argparse.ArgumentParser()
    p.add_argument("--enabled", action="store_true", default="true")
    p.add_argument("--disabled", action="store_false", default="false")
    auto ns = p.parse_args(["--enabled", "--disabled"])
    print(ns.get_flag("enabled"))
    print(ns.get_flag("disabled"))
    print(ns.has("enabled"))
    print(ns.has("disabled"))


def supplying_against_true_default():
    # store_true with default="true" stays true when supplied; store_false with
    # default="false" stays false when supplied (action still runs).
    auto p = argparse.ArgumentParser()
    p.add_argument("--enabled", action="store_true", default="true")
    p.add_argument("--disabled", action="store_false", default="false")
    auto ns = p.parse_args(["--enabled", "--disabled"])
    print(ns.get_flag("enabled"))
    print(ns.get_flag("disabled"))


def flag_default_consumes_no_value():
    # A trailing positional after a defaulted flag binds to the positional, not
    # the flag (the flag is zero-arity, #1508).
    auto p = argparse.ArgumentParser()
    p.add_argument("--enabled", action="store_true", default="true")
    p.add_argument("target")
    auto ns = p.parse_args(["--enabled", "out.txt"])
    print(ns.get_flag("enabled"))
    print(ns.get("target"))
    print(ns.has("enabled"))


def invalid_default_raises():
    auto p = argparse.ArgumentParser()
    try:
        p.add_argument("--enabled", action="store_true", default="yes")
        print("no-error")
    except ValueError:
        print("invalid-default-caught")


def invalid_default_store_false_raises():
    auto p = argparse.ArgumentParser()
    try:
        p.add_argument("--disabled", action="store_false", default="1")
        print("no-error")
    except ValueError:
        print("invalid-default-caught")


def non_flag_default_untouched():
    # A value-taking option with default="true" is a plain string default; the
    # boolean normalisation applies only to flags.
    auto p = argparse.ArgumentParser()
    p.add_argument("--label", default="true")
    auto ns = p.parse_args([])
    print(ns.get("label"))
    print(ns.has("label"))


def main():
    action_defaults_when_omitted()
    explicit_defaults_preserved()
    explicit_default_matching_action()
    supplying_overrides_default()
    supplying_against_true_default()
    flag_default_consumes_no_value()
    invalid_default_raises()
    invalid_default_store_false_raises()
    non_flag_default_untouched()
