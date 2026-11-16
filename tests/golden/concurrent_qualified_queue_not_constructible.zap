# I#996, D-0202 (negative): the seam sugar fires ONLY in constructor-call
# position. A bare `concurrent.Queue` used as a value is not a binding and is
# rejected — module-qualified construction is reached through a call, not by
# naming the dotted path as a value.
import concurrent

def main():
    auto q = concurrent.Queue
    print(1)
