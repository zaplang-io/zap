# diff: match
# An uncaught exception terminates the program with a non-zero exit status,
# matching CPython (epic #374, walled garden): output produced before the raise
# is flushed, then the process exits 1. The diff harness compares stdout and exit
# status (not the traceback text, which renders differently), so this pins
# "before" on stdout and exit 1 on both engines.
def main():
    print("before")
    raise ValueError("boom")
