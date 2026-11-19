# Borrowed-bytes GC soundness (#1082 follow-up): a bytesview held across a GC
# safepoint must survive a moving-Immix evacuation of its backing, exactly like
# strview — the view carries its `bytes` backing as a traced interior pointer
# (the 2b base+offset representation, #586), rewritten on a move. Contents only.
def churn(int64 n):
    for i in range(0, n):
        auto g = [i, i + 1, i + 2]

def main():
    bytes b = bytes([98, 111, 114, 114, 111, 119])
    bytesview v = b.view(0, 3)
    churn(200000)
    gc.collect()
    gc.collect()
    print(len(v))  # 3
    print(v == bytes([98, 111, 114]))  # True
    print(v.startswith(bytes([98])))  # True
