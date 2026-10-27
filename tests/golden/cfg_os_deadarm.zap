# Dead-arm skip (I#139 / D-0079): the Windows arm calls a function that does
# not exist. On every host that runs the golden suite (Linux x86_64/aarch64)
# that arm is dead and is skipped — parsed, but never name-resolved or
# type-checked — so the program still compiles and the `else` arm runs.
when target.os == .windows:
    def sep() -> str:
        return windows_only_path_separator()
else:
    def sep() -> str:
        return "/"

def main():
    print(sep())
