# Statement-position `when target.*` (I#139 / D-0079): the Windows arm is a
# dead arm on the Linux golden hosts and is skipped, so the `else` branch's
# binding is the one that survives.
def label() -> str:
    when target.os == .windows:
        auto s = windows_label()
    else:
        auto s = "posix"
    return s

def main():
    print(label())
