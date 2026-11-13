# size > cap with RUNTIME values (not constant-foldable, so past the ZAP5132
# compile-time check) raises at arena() open (#553, D-0158) — caught here so the
# program is deterministic under both collectors.
def val(int64 x) -> int64:
    return x

def main():
    auto caught = 0
    try:
        with arena(size=val(131072), cap=val(65536)) as a:
            pass
    except Exception:
        caught = 1
    print(caught)
