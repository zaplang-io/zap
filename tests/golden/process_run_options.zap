# Golden fixture: process.run options (#976, D-0237) — env override/augment,
# cwd, input=, capture toggles, and stderr->stdout merge. POSIX paths (CI is
# Linux); `printf` (no trailing newline) keeps every captured string single-line.
def main():
    # input= feeds stdin via a temp file; cat echoes it back on stdout.
    auto c1, o1, e1 = process.run(["/bin/cat"], input="hello")
    print(c1)  # 0
    print(o1)  # hello
    print(len(e1))  # 0

    # env override/augment: the child sees ZAPX; the shell echoes just that var.
    auto env = {"ZAPX": "forty-two"}
    auto c2, o2, e2 = process.run_shell("printf %s $ZAPX", env=env)
    print(o2)  # forty-two
    print(c2)  # 0

    # cwd: run the shell's pwd in "/".
    auto c3, o3, e3 = process.run_shell("printf %s \"$(pwd)\"", cwd="/")
    print(o3)  # /

    # merge_stderr folds the child's stderr into stdout (err stays empty).
    auto c4, o4, e4 = process.run_shell(
        "printf out; printf err 1>&2",
        merge_stderr=True,
    )
    print(o4)  # outerr
    print(len(e4))  # 0

    # capture_output=False: nothing is captured (both strings empty); /bin/true
    # produces no output, so nothing leaks to the inherited stdout.
    auto c5, o5, e5 = process.run(["/bin/true"], capture_output=False)
    print(c5)  # 0
    print(len(o5))  # 0
    print(len(e5))  # 0
