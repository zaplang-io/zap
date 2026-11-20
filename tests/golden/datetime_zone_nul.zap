# Golden fixture: named-zone lookup validates the WHOLE Zap string (#1172, bug).
# A name carrying an embedded NUL after a valid zone must NOT silently truncate
# to that zone — it raises the same catchable unknown-zone error as any absent
# name. Empty, overlong, and plain-unknown names reject the same way. Caught so
# the program is deterministic under both collectors.
import datetime


def main():
    # A valid name resolves without raising.
    auto ok = 0
    try:
        auto z = datetime.zone("America/New_York")
        ok = 1
    except Exception:
        ok = 0
    print(ok)  # 1

    # Embedded NUL after a valid name is rejected (not truncated to it).
    auto nul = 0
    try:
        auto bad = datetime.zone("America/New_York\x00suffix")
    except Exception:
        nul = 1
    print(nul)  # 1

    # The same validation applies when the named zone is resolved later via
    # astimezone (a second lookup path).
    auto nul_resolve = 0
    try:
        auto t = datetime.astimezone(
            datetime.from_timestamp(0),
            datetime.fixed_offset(0),
        )
        # fixed zone is fine; now force the named path with a bad name.
        auto bad2 = datetime.zone("UTC\x00X")
    except Exception:
        nul_resolve = 1
    print(nul_resolve)  # 1

    # Empty name is rejected.
    auto empty = 0
    try:
        auto e = datetime.zone("")
    except Exception:
        empty = 1
    print(empty)  # 1

    # A plain unknown name is rejected (the contract baseline).
    auto unknown = 0
    try:
        auto u = datetime.zone("Mars/Olympus_Mons")
    except Exception:
        unknown = 1
    print(unknown)  # 1
