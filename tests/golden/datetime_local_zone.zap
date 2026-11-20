# Golden fixture: datetime.local_zone() exposes the host system zone as a
# reusable NAMED TimeZone (#1171, ADR decisions/stdlib/1171-system-local-zone.md).
# The `.env` sidecar pins TZ=America/New_York so the output is deterministic. The
# SAME handle must yield EST (-05:00) in January and EDT (-04:00) in July — proof
# it re-resolves the zone's rules per instant rather than snapshotting one offset.
import datetime


def main():
    auto z = datetime.local_zone()
    # Fixed UTC instants: 2024-01-15T12:00:00Z and 2024-07-15T12:00:00Z.
    auto jan = datetime.astimezone(datetime.from_timestamp(1705320000), z)
    auto jul = datetime.astimezone(datetime.from_timestamp(1721044800), z)
    print(jan.offset)  # -18000 (EST)
    print(jul.offset)  # -14400 (EDT)
    print(datetime.isoformat(jan))  # 2024-01-15T07:00:00-05:00
    print(datetime.isoformat(jul))  # 2024-07-15T08:00:00-04:00
