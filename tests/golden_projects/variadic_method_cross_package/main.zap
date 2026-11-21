# A program importing a dependency package by its logical name (D-0052) and
# calling a variadic struct method across the package boundary (#1218). The
# dependency resolves through `$ZAP_ZENV/pkg`, not the project's own src root,
# so the collected-list argument is assembled at a call site in the consuming
# package while the callee lives in the installed `memcache` package — the
# importing-module case from the bug.
import memcache

def main():
    memcache.Client c = memcache.connect(7)
    print(c.stats())
    print(c.stats(b"items"))
    print(c.stats(b"items", b"slabs"))
