# A module `const` is a module member (D-0112, #658 Phase 1): reachable across
# modules as `conf.LIMIT`, and mangled so it never collides with a same-named
# const in the importer. Initialized before `main`, in import-topological order
# (this dependency's const initializes before the entry's).
const int64 LIMIT = 100

def cap() -> int64:
    return LIMIT
