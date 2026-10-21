# unwrap() on an Optional.None raises (string message until typed exceptions,
# I#12). The exception is uncaught, so the process aborts non-zero after the
# "before" line and never reaches "after" (I#154, D-0064).
def main():
    Optional[int64] n = None
    print("before")
    print(n.unwrap())
    print("after")
