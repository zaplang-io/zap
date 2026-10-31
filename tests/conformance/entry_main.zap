# claim: ENTRY-1
# SPEC §2 (D-0021/D-0090): executable code runs inside `main`; a bare module
# docstring is metadata, not an executable top-level statement.
"Entry point conformance case."

def main():
    print("main")
