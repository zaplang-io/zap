# Consumer: iterate a generator method that lives in the imported `pager`
# module and yields a `Page` struct, reading a field off each yielded struct.
# Expected 4, 9 (same page starts as the control). Observed before the #451 fix:
# empty stdout — the program produced no output at all.
import pager

def main():
    auto p = pager.Pager(total=10, page_size=3)
    auto count = 0
    auto last = 0
    for pg in p.pages(0):
        count = count + 1
        last = pg.start
    print(count)
    print(last)
