# Golden fixture: `datetime.strftime` / 2-arg `datetime.format` custom formats
# (ticket #286, D-0056).
#
# Driven from a FIXED Unix timestamp so output is fully deterministic and
# timezone-independent: 1700000000 = 2023-11-14T22:13:20Z (a Tuesday), built in
# UTC. Covers the SigV4 timestamp formats that surfaced this issue plus a
# representative slice of the Python-faithful directive set. No `now()` here.

def main():
    auto dt = datetime.from_timestamp(1700000000)

    # SigV4: X-Amz-Date (basic ISO) and the credential-scope date.
    print(datetime.strftime(dt, "%Y%m%dT%H%M%SZ"))  # 20231114T221320Z
    print(datetime.strftime(dt, "%Y%m%d"))  # 20231114

    # The 2-arg `format` overload is the same formatter.
    print(datetime.format(dt, "%Y-%m-%d %H:%M:%S"))  # 2023-11-14 22:13:20

    # Names, 12-hour clock, AM/PM, day-of-year, 2-digit year.
    print(datetime.strftime(dt, "%A, %d %B %Y %I:%M %p"))  # Tuesday, 14 November 2023 10:13 PM
    print(datetime.strftime(dt, "%a %b %j %y"))  # Tue Nov 318 23

    # Naive DateTime: %Z / %z expand empty (Python-faithful); %% is a literal.
    print(datetime.strftime(dt, "|%Z|%z| 100%%"))  # ||| 100%

    # One-arg `format` is still ISO-8601 extended.
    print(datetime.format(dt))  # 2023-11-14T22:13:20

    # Midnight and noon on the 12-hour clock.
    auto midnight = datetime.from_timestamp(1699920000)
    print(datetime.strftime(midnight, "%H:%M %I%p"))  # 00:00 12AM
