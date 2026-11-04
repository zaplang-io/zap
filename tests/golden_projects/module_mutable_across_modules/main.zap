import state

# A same-named mutable global in the importer stays independent of state.hits.
int64 hits = 1000

def main():
    print(state.record())  # 1  — dependency's global mutated
    print(state.record())  # 2
    print(state.hits)  # 2  — read the dependency's global across modules
    print(hits)  # 1000 — importer's own global, untouched
    hits = hits + 1
    print(hits)  # 1001 — importer's global reassigned, persisted
