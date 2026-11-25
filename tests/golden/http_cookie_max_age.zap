# Golden (#1524): `http.parse_set_cookie` max_age is `Optional[int64]`, so a
# caller can tell an ABSENT or malformed `Max-Age` (`None`) apart from a valid
# parsed one (`Some(n)`), including the non-positive values RFC 6265 §5.2.2
# treats as immediate deletion. Before the fix both an absent attribute and a
# wire `Max-Age=-1` surfaced as the int64 `-1`, so the two were indistinguishable.
#
# Cases, each printed as a distinct line:
#   * omitted          -> None        ("absent")
#   * malformed         -> None        ("absent")
#   * Max-Age=-1        -> Some(-1)    (delete now)
#   * Max-Age=0         -> Some(0)     (delete now)
#   * Max-Age=3600      -> Some(3600)  (expire in an hour)
# Plus the precedence note: a valid `Max-Age=-1` ALONGSIDE a future `Expires`
# must still expose `Some(-1)` so the caller can let Max-Age win (delete now),
# and `expires` is still the raw date string.

def show(str label, str header):
    auto ck = http.parse_set_cookie(header)
    if ck.max_age is None:
        print(label + ": absent")
    else:
        print(label + ": " + str(ck.max_age.unwrap()))


def main():
    show("omitted", "id=one; Path=/")
    show("malformed", "id=one; Path=/; Max-Age=later")
    show("empty", "id=one; Max-Age=")
    show("neg_one", "id=one; Max-Age=-1")
    show("zero", "id=one; Max-Age=0")
    show("positive", "id=one; Max-Age=3600")

    # Precedence: Max-Age=-1 wins over a future Expires — the typed result keeps
    # both, so the caller can apply RFC 6265 §5.2.2 (delete now) without
    # reparsing the header.
    auto ck = http.parse_set_cookie(
        "id=one; Max-Age=-1; Expires=Wed, 21 Oct 2099 07:28:00 GMT",
    )
    if ck.max_age is None:
        print("precedence: absent")
    else:
        print(
            "precedence: max_age=" + str(ck.max_age.unwrap()) + " expires=" + ck.expires,
        )
    print("done")
