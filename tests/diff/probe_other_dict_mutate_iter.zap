# diff: diverge D-0130: `for k in d` iterates a key snapshot, so mutation during the loop never raises (CPython: RuntimeError). Documented divergence (#374/#743).
# PROBE (epic #374, §10 "other"): deleting a key while iterating a dict. CPython
# detects the size change and raises a catchable `RuntimeError`, so the handler
# runs and prints `caught` (observation is order-independent: any mutation trips
# it). If Zap allows the mutation or crashes, the report shows the behavior.
# CPython prints:
#   caught
def main():
    dict[int64, int64] d = {1: 10, 2: 20, 3: 30}
    try:
        for k in d:
            del d[k]
        print("no-error")
    except Exception as e:
        print("caught")
