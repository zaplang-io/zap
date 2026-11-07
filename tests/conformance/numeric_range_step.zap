# claim: CTRL-5
# SPEC §6 (#374, #759): `range(a, b, step)` honors the step sign — a negative
# step counts down — and a zero step raises a catchable ValueError (before the
# loop runs), rather than looping forever.
def main():
    auto a = 0
    for i in range(5, 0, -1):
        a = a + i
    print(a)
    auto b = 0
    for i in range(0, 10, 3):
        b = b + i
    print(b)
    try:
        for i in range(0, 5, 0):
            print(i)
    except ValueError:
        print("zero step")
