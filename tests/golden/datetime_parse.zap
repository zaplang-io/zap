# Golden fixture: `datetime.strptime` parsing + the AWS format/parse round-trips
# (ticket #338, builds on the #286 strftime machinery). Deterministic and UTC.
#
# Fixed instants used below:
#   1700000000 = 2023-11-14T22:13:20Z   (a Tuesday)
#   1699920000 = 2023-11-14T00:00:00Z   (same day, midnight — the credential-scope date)
#    784111777 = 1994-11-06T08:49:37Z   (the classic RFC-822 / HTTP-date example)

def main():
    auto dt = datetime.from_timestamp(1700000000)

    # --- SigV4 basic: `%Y%m%dT%H%M%SZ` --- format then parse back.
    auto sig = datetime.strftime(dt, "%Y%m%dT%H%M%SZ")
    print(sig)  # 20231114T221320Z
    print(datetime.to_timestamp(datetime.strptime(sig, "%Y%m%dT%H%M%SZ")))  # 1700000000

    # Parse the literal SigV4 example date string.
    print(
        datetime.to_timestamp(datetime.strptime("20260101T000000Z", "%Y%m%dT%H%M%SZ")),
    )  # 1767225600

    # --- SigV4 credential-scope date-only: `%Y%m%d` (packed, no separators).
    print(datetime.strftime(dt, "%Y%m%d"))  # 20231114
    print(datetime.to_timestamp(datetime.strptime("20231114", "%Y%m%d")))  # 1699920000

    # --- iso8601 / RFC-3339 with the `Z`: the AWS `date-time` form.
    auto iso = datetime.strftime(dt, "%Y-%m-%dT%H:%M:%SZ")
    print(iso)  # 2023-11-14T22:13:20Z
    print(datetime.to_timestamp(datetime.strptime(iso, "%Y-%m-%dT%H:%M:%SZ")))  # 1700000000

    # --- rfc822 / HTTP-date: the HTTP `Date` header form.
    auto http = datetime.from_timestamp(784111777)
    auto rfc = datetime.strftime(http, "%a, %d %b %Y %H:%M:%S GMT")
    print(rfc)  # Sun, 06 Nov 1994 08:49:37 GMT
    print(datetime.to_timestamp(datetime.strptime(rfc, "%a, %d %b %Y %H:%M:%S GMT")))  # 784111777

    # --- epoch seconds (AWS `timestampFormat: unixTimestamp`) round-trip.
    print(datetime.to_timestamp(datetime.from_timestamp(1700000000)))  # 1700000000
    # Fractional unixTimestamp truncates to whole seconds (DateTime's resolution).
    print(datetime.to_timestamp(datetime.from_timestamp(1700000000.9)))  # 1700000000
