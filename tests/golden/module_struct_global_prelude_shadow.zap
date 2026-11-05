# Regression for #689: a user struct named `Counter` fires the `"Counter"`
# prelude needle, pulling in the `concurrent` module. The prelude's `Counter`
# struct is shadowed by the user's, and its dependent helper
# `__concurrent_counter` (which builds `Counter(_cell=…)`) is dropped too, so
# member access resolves against the user's struct instead of failing at
# lowering with `no field '_cell' on struct`.
struct Counter:
    int64 n

    def get(self) -> int64:
        return self.n

Counter c = Counter(5)

def main():
    print(c.n)  # 5
    print(c.get())  # 5
