# Golden fixture: `@range` applies to a numeric field only (#472, D-0101).
# Attaching it to a `str` field is a stable, coded compile error (ZAP5039).

struct S:
    @range(min=0, max=5)
    str name

def main():
    print(1)
