# Golden fixture: Perl-style regex (ticket #23, D-0010) via PCRE2.
# Exercises m// match (truthy), positional + named capture groups, the `!~`
# non-match test, and s/// substitution (first + global), all returning a match
# object / new string with no magic globals (coroutine-safe captures).
def main():
    auto text = "2026-10-18"

    # Match with positional capture groups: group(0) is the whole match.
    auto mt = text =~ m/(\d{4})-(\d{2})-(\d{2})/
    if mt:
        print(mt.group(0))
        print(mt.group(1))
        print(mt.group(2))
        print(mt.group(3))

    # Named capture group, by name.
    auto d = "ip=10.0.0.7" =~ m/(?<ip>\d+\.\d+\.\d+\.\d+)/
    if d:
        print(d.group("ip"))

    # `!~` is true when there is no match.
    if "hello" !~ m/\d+/:
        print("no digits")

    # Substitution returns a new string; `g` replaces every occurrence.
    auto collapsed = "a   b    c" =~ s/\s+/ /g
    print(collapsed)

    # Replacement can reference capture groups ($1, $2); no `g` -> first only.
    auto swapped = "John Smith" =~ s/(\w+)\s+(\w+)/$2 $1/
    print(swapped)

    # In-place substitution shorthand (`~=`): desugars to label = label =~ s///.
    str label = "hot dog"
    label ~= s/\s+/-/g
    print(label)
