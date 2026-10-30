# Consumer: iterate a generator method defined in the imported `pager` module.
# Mirrors the single-file control exactly (counts 4, last 9) — the only
# difference is the import boundary.
import pager

def main():
    auto p = pager.Pager(total=10, page_size=3)
    auto count = 0
    auto last = 0
    for start in p.pages(0):
        count = count + 1
        last = start
    print(count)
    print(last)
