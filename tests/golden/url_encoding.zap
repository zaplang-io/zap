# Golden fixture: encoding/url — RFC-3986 percent-encoding + decoding and the
# AWS SigV4 query canonicalization variant (ticket #335). Covers unreserved
# pass-through, space + reserved escaping, UTF-8 multibyte (encoded byte-by-byte
# over the UTF-8 encoding), the path (keep-'/') form, lenient decode, a
# quote/unquote round-trip, and a real AWS canonical-query vector (supplied out
# of order, returned sorted and encoded).
import url

def main():
    # Unreserved set (RFC-3986 §2.3: A-Z a-z 0-9 - . _ ~) passes through.
    print(url.quote("AZaz09-._~"))  # AZaz09-._~

    # Space -> %20 (never '+') and reserved chars (incl. '/') are escaped,
    # UPPERCASE hex.
    print(url.quote("a b/c?d=e&f"))  # a%20b%2Fc%3Fd%3De%26f

    # quote_path keeps '/' as a path separator but still escapes the space.
    print(url.quote_path("/foo bar/baz"))  # /foo%20bar/baz

    # UTF-8 multibyte: each UTF-8 byte is percent-encoded. 'é' = C3 A9,
    # '€' = E2 82 AC.
    print(url.quote("café €"))  # caf%C3%A9%20%E2%82%AC

    # Round-trip: unquote is the exact inverse of quote over arbitrary bytes.
    auto s = "café / 100% built-in?"
    if url.unquote(url.quote(s)) == s:
        print("roundtrip-ok")
    else:
        print("roundtrip-FAIL")

    # Decode is lenient: a malformed/truncated '%' escape stays literal, and a
    # bare '+' is NOT a space under unquote.
    print(url.unquote("%zz-%2-a+b"))  # %zz-%2-a+b

    # unquote_plus additionally maps '+' -> space (form-encoded values).
    print(url.unquote_plus("a+b%20c"))  # a b c

    # encode_query builds "k=v" (dict order is unspecified, so a single pair
    # here keeps the fixture deterministic).
    auto one = {"q": "a b&c"}
    print(url.encode_query(one))  # q=a%20b%26c

    # canonical_query: SigV4 CanonicalQueryString. The params are supplied out
    # of order and come back sorted by encoded key (real AWS test-suite vector).
    auto ordered = {"Param2": "value2", "Param1": "value1"}
    print(url.canonical_query(ordered))  # Param1=value1&Param2=value2

    # A query-protocol request (STS ListUsers) whose value needs escaping; keys
    # sort A < D < V, the '/' and ' ' in the value become %2F and %20.
    auto sts = {"Version": "2011-06-15", "Action": "ListUsers", "Delimiter": "a/b c"}
    print(url.canonical_query(sts))
    # Action=ListUsers&Delimiter=a%2Fb%20c&Version=2011-06-15
