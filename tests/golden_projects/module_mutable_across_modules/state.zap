# A mutable global is a module member (D-0112, #662): reachable across modules
# as `state.hits`, mangled so it never collides with a same-named global in the
# importer, and initialized before `main` in import-topological order (this
# dependency's global initializes before the entry's).
int64 hits = 0

def record() -> int64:
    hits += 1
    return hits
