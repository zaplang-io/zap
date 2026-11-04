# @shareable reflection (#664, D-0113). The concurrency-safety marker is
# reflectable just like any other annotation: `reflect_has(T, "shareable")` is
# true for a user `struct` the author marked `@shareable` (via the runtime
# reflection registry), true for the built-in concurrency primitives and
# concurrent collections that carry the marker implicitly (a compile-time fold,
# so they need no registry entry), and false for an unmarked type or an absent
# marker. Mirrors the query style of `decorators.zap` / `fn_decorator_reflect.zap`.

@shareable
struct SafeBox:
    int64 n

struct PlainBox:
    int64 n

def main():
    if reflect_has("SafeBox", "shareable"):
        print("SafeBox shareable")
    if not reflect_has("PlainBox", "shareable"):
        print("PlainBox not shareable")
    if reflect_has("Atomic", "shareable"):
        print("Atomic shareable")
    if reflect_has("Mutex", "shareable"):
        print("Mutex shareable")
    if reflect_has("Counter", "shareable"):
        print("Counter shareable")
    if not reflect_has("SafeBox", "nonexistent"):
        print("SafeBox no nonexistent")
