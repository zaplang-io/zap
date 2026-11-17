# Golden fixture: encoding/url — URL parse / build / query-reading / reference
# resolution (ticket #970, D-0209). Exhaustive companion to `url_encoding`
# (the percent codec). Covers:
#   A. url.split decomposition — scheme/userinfo/host/port/path/query/fragment,
#      IPv6 hosts, empty authority, scheme-relative + path-only refs, scheme
#      case-folding, absent port (-1).
#   B. build(split(s)) round-trips for normalized inputs.
#   C. url.build from a hand-built UrlParts (incl. IPv6 bracket re-wrapping).
#   D. url.parse_query — repeated keys, blank values, '+'/%XX decoding, lenient
#      malformed escapes, empty + empty-segment inputs.
#   E. url.parse_query_first — first value per key.
#   F. url.encode_query_multi — repeated-key encoder (single key => deterministic).
#   G. url.join — the RFC-3986 §5.4 normal + abnormal reference vectors.
#   H. an invalid port raises a catchable ValueError (eager, at split time).
import url


def main():
    # -- A. split --------------------------------------------------------------
    # Full authority: userinfo (user:pass), host, port, path, query, fragment.
    auto a = url.split("https://user:pass@example.com:8080/path/to?x=1&y=2#frag")
    print(
        f"{a.scheme}|{a.has_authority}|{a.userinfo}|{a.host}|{a.port}|{a.path}|{a.query}|{a.fragment}",
    )
    # IPv6 literal host: stored WITHOUT brackets, port after "]:".
    auto b = url.split("http://[::1]:9000/")
    print(
        f"{b.scheme}|{b.has_authority}|{b.userinfo}|{b.host}|{b.port}|{b.path}|{b.query}|{b.fragment}",
    )
    # No "//" => no authority; the '@' belongs to the path, port is -1.
    auto c = url.split("mailto:user@example.com")
    print(
        f"{c.scheme}|{c.has_authority}|{c.userinfo}|{c.host}|{c.port}|{c.path}|{c.query}|{c.fragment}",
    )
    # Scheme-relative reference ("//host/...").
    auto d = url.split("//cdn.example.com/lib.js")
    print(
        f"{d.scheme}|{d.has_authority}|{d.userinfo}|{d.host}|{d.port}|{d.path}|{d.query}|{d.fragment}",
    )
    # Path-only (no scheme, no authority).
    auto e = url.split("/a/b/c")
    print(
        f"{e.scheme}|{e.has_authority}|{e.userinfo}|{e.host}|{e.port}|{e.path}|{e.query}|{e.fragment}",
    )
    # Empty authority ("file:///path"): has_authority stays True, host empty.
    auto f = url.split("file:///etc/hosts")
    print(
        f"{f.scheme}|{f.has_authority}|{f.userinfo}|{f.host}|{f.port}|{f.path}|{f.query}|{f.fragment}",
    )
    # Scheme is case-folded to lowercase; the host keeps its case (netloc form).
    auto g = url.split("HTTPS://Example.COM/Path")
    print(
        f"{g.scheme}|{g.has_authority}|{g.userinfo}|{g.host}|{g.port}|{g.path}|{g.query}|{g.fragment}",
    )
    # Authority, no path / query / fragment.
    auto h = url.split("https://example.com")
    print(
        f"{h.scheme}|{h.has_authority}|{h.userinfo}|{h.host}|{h.port}|{h.path}|{h.query}|{h.fragment}",
    )

    # -- B. build(split(s)) round-trips (normalized inputs) --------------------
    print(
        url.build(url.split("https://user:pass@example.com:8080/path/to?x=1&y=2#frag")) == "https://user:pass@example.com:8080/path/to?x=1&y=2#frag",
    )
    print(url.build(url.split("http://[::1]:9000/")) == "http://[::1]:9000/")
    print(url.build(url.split("mailto:user@example.com")) == "mailto:user@example.com")
    print(
        url.build(url.split("//cdn.example.com/lib.js")) == "//cdn.example.com/lib.js",
    )
    print(url.build(url.split("/a/b/c")) == "/a/b/c")
    print(url.build(url.split("file:///etc/hosts")) == "file:///etc/hosts")
    print(url.build(url.split("https://example.com")) == "https://example.com")
    print(url.build(url.split("http://example.com/")) == "http://example.com/")

    # -- C. build from a hand-built UrlParts -----------------------------------
    auto p1 = UrlParts(
        scheme="https",
        has_authority=True,
        userinfo="",
        host="example.com",
        port=443,
        path="/search",
        query="q=zap",
        fragment="",
    )
    print(url.build(p1))  # https://example.com:443/search?q=zap
    # IPv6 host (unbracketed) is re-wrapped in "[...]" on build.
    auto p2 = UrlParts(
        scheme="http",
        has_authority=True,
        userinfo="",
        host="::1",
        port=-1,
        path="/",
        query="",
        fragment="",
    )
    print(url.build(p2))  # http://[::1]/
    # userinfo + fragment.
    auto p3 = UrlParts(
        scheme="ftp",
        has_authority=True,
        userinfo="anon",
        host="h",
        port=-1,
        path="/f",
        query="",
        fragment="top",
    )
    print(url.build(p3))  # ftp://anon@h/f#top

    # -- D. parse_query (dict[str, list[str]]) ---------------------------------
    # Repeated key accumulates; a key with no '=' and an empty value are KEPT
    # with an empty string (Go url.ParseQuery form, blank-value-friendly).
    auto q = url.parse_query("a=1&b=2&a=3&c&d=")
    print(len(q["a"]))  # 2
    print(q["a"][0])  # 1
    print(q["a"][1])  # 3
    print(q["b"][0])  # 2
    print(len(q["c"]))  # 1
    auto cv = q["c"][0]
    print(f"[{cv}]")  # [] (blank)
    auto dv = q["d"][0]
    print(f"[{dv}]")  # [] (blank)
    # '+' -> space and %XX decoding (incl. UTF-8 multibyte).
    auto q2 = url.parse_query("name=John+Doe&city=S%C3%A3o%20Paulo")
    print(q2["name"][0])  # John Doe
    print(q2["city"][0])  # São Paulo
    # A malformed/truncated %XX is left literal (lenient, like url.unquote).
    auto q3 = url.parse_query("x=%zz&y=%2")
    print(q3["x"][0])  # %zz
    print(q3["y"][0])  # %2
    # Empty query and empty segments.
    print(len(url.parse_query("")))  # 0
    auto q4 = url.parse_query("&a=1&&b=2&")
    print(len(q4))  # 2
    print(q4["a"][0])  # 1
    print(q4["b"][0])  # 2

    # -- E. parse_query_first (dict[str, str]) ---------------------------------
    auto r = url.parse_query_first("a=1&a=2&b=3")
    print(r["a"])  # 1
    print(r["b"])  # 3

    # -- F. encode_query_multi (single key => deterministic order) -------------
    print(url.encode_query_multi({"tag": ["x", "y", "z"]}))  # tag=x&tag=y&tag=z
    print(url.encode_query_multi({"q": ["a b&c"]}))  # q=a%20b%26c
    # Round-trips a real dict[str, list[str]] straight from parse_query.
    print(url.encode_query_multi(url.parse_query("m=1&m=2")))  # m=1&m=2

    # -- G. join — RFC-3986 §5.4 reference-resolution vectors ------------------
    auto base = "http://a/b/c/d;p?q"
    print(url.join(base, "g"))  # http://a/b/c/g
    print(url.join(base, "./g"))  # http://a/b/c/g
    print(url.join(base, "g/"))  # http://a/b/c/g/
    print(url.join(base, "/g"))  # http://a/g
    print(url.join(base, "//g"))  # http://g
    print(url.join(base, "?y"))  # http://a/b/c/d;p?y
    print(url.join(base, "g?y"))  # http://a/b/c/g?y
    print(url.join(base, "#s"))  # http://a/b/c/d;p?q#s
    print(url.join(base, "g#s"))  # http://a/b/c/g#s
    print(url.join(base, ""))  # http://a/b/c/d;p?q
    print(url.join(base, ".."))  # http://a/b/
    print(url.join(base, "../g"))  # http://a/b/g
    print(url.join(base, "../.."))  # http://a/
    print(url.join(base, "../../g"))  # http://a/g
    print(url.join(base, "../../../g"))  # http://a/g  (excess ".." stays at root)
    print(url.join(base, "http://x/y/z"))  # http://x/y/z  (absolute ref wins)

    # -- H. an invalid port raises a catchable ValueError ----------------------
    try:
        auto bad = url.split("http://h:99999/")
        print(f"no-raise:{bad.port}")
    except Exception:
        print("bad-port-caught")
    try:
        auto bad2 = url.split("http://h:abc/")
        print(f"no-raise:{bad2.port}")
    except Exception:
        print("nonnumeric-port-caught")
