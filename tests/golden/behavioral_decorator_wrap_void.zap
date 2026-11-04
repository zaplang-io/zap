# Golden fixture: a `wrap(call)` decorator on a function with no return type
# (D-0036; #653).
#
# `@around` runs code before and after the original, calling it as an
# expression statement (`call(*args)`). The decorated `act` returns nothing, so
# the generated wrapper has a void signature and NO fall-through guard is
# emitted (the guard exists only for a declared return type).

decorator around:
    def wrap(call):
        def inner(*args):
            print("before")
            call(*args)
            print("after")
        return inner

@around
def act(str msg):
    print("act", msg)

def main():
    act("hi")
