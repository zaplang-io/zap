# Python runtime interop (ticket #27, D-0012, design/0003-python-interop.md):
# embed CPython, import stdlib modules, call into them, and marshal values
# across the Zap <-> Python boundary. Stdlib-only (math, json) so it runs in the
# container with no pip install.
import python "math" as m
import python "json" as j

def main():
    # int marshaling both ways: Zap int64 -> Python int args, Python int result
    # -> Zap int64 via an explicit `int64(...)` conversion.
    int64 g = int64(m.gcd(48, 60))
    print(g)
    # float marshaling: a Zap float64 argument, and a Python float result printed
    # via its str().
    print(m.sqrt(2.0))
    # str marshaling: a Zap str crosses into Python and the str result crosses
    # back (json.dumps quotes the string).
    print(j.dumps("hello"))
