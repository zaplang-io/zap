# Golden fixture: soft (contextual) keywords (#1405 `scope`; #1411 `select`,
# `match`, `case`; epic #1413). Each word is a keyword ONLY in its statement /
# clause slot and an ordinary identifier everywhere else. This program uses all
# four as a struct field, a function parameter, a named-argument label, an
# attribute read and a plain local variable — in the SAME program as the
# `match`/`case`, `scope` and `select` statement forms — and runs to a
# deterministic result (single-producer scope join + a pre-filled select).


struct Token:
    str scope
    str match
    str select
    str case


def tag(str match, str case) -> str:
    # `match` and `case` as ordinary parameters and expressions.
    return match + "/" + case


def deposit(channel[str] ch, str v):
    ch <- v


def main():
    # Named-argument labels that are soft keywords.
    auto t = Token(scope="read", match="exact", select="id", case="a")
    # Attribute reads whose names are soft keywords.
    print(t.scope)  # read
    print(t.match)  # exact
    print(t.select)  # id
    print(t.case)  # a
    print(tag(match=t.match, case=t.case))  # exact/a

    # `match` / `case` KEYWORD form over an attribute literally named `match`.
    match t.match:
        case "exact":
            print("hit")  # hit
        case _:
            print("miss")

    # `scope` and `select` remain live local variables.
    auto scope = t.select
    auto select = t.scope
    print(scope + "-" + select)  # id-read

    # `scope` KEYWORD form (structured concurrency) alongside the `scope`
    # variable above; the single worker's result is drained after the join, so
    # the output is deterministic.
    auto ch = channel[str](1)
    scope:
        spawn deposit(ch, scope)
    print(<-ch)  # id

    # `select` KEYWORD form alongside the `select` variable above; the channel
    # is pre-filled and the sole case is ready, so the result is deterministic.
    auto sc = channel[str](1)
    sc <- select
    select:
        case got = <-sc:
            print(got)  # read
