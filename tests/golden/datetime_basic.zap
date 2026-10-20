# Golden fixture: the `datetime` module (ticket #99, D-0056).
#
# Driven from a FIXED Unix timestamp so the output is fully deterministic and
# timezone-independent: all conversions here are UTC. 1700000000 =
# 2023-11-14T22:13:20Z. Also exercises the civil<->timestamp round trip and
# ISO-8601 parse. Identical under both collectors.

def main():
    auto dt = datetime.from_timestamp(1700000000)
    print(dt.year)                 # 2023
    print(dt.month)                # 11
    print(dt.day)                  # 14
    print(dt.hour)                 # 22
    print(dt.minute)               # 13
    print(dt.second)               # 20
    print(datetime.format(dt))     # 2023-11-14T22:13:20

    # Civil -> timestamp round trip (UTC).
    print(datetime.to_timestamp(dt))   # 1700000000

    # Parse ISO-8601 back to the same instant.
    auto p = datetime.parse("2023-11-14T22:13:20")
    print(datetime.to_timestamp(p))    # 1700000000
