# Compile-time architecture selection (`when target.*`, I#139 / D-0079).
# Each arm defines `kind` for its target; cfg resolution keeps exactly one, so
# there is never a duplicate definition. All arms print the same text, so the
# program's output is target-independent (x86_64 and aarch64 both print "ok").
when target.arch == .x86_64:
    def kind() -> str:
        return "ok"
elif target.arch == .aarch64:
    def kind() -> str:
        return "ok"
else:
    def kind() -> str:
        return "ok"

def main():
    print(kind())
