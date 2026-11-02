# Regression for #564: an omitted `Optional[T]` argument whose declared default
# is `None` must materialize the tagged `Optional[T].None`, not a raw null
# pointer, so `is_some()`/`is_none()`/`unwrap_or` behave instead of dereferencing
# nil at native runtime. Covers a free function and a struct method; an omitted
# positional argument, an omitted/explicit keyword `None`, an explicitly-passed
# `None`, and a typed `Optional[T]` None value passed in; plus a pointer payload
# (`Optional[str]`) to show the fix is general across every `Optional[T]`, not
# int64-only. (Before the fix, `present()` crashed with SIGSEGV in
# `Optional$int64__is_some`.)

def present(Optional[int64] value = None) -> bool:
    return value.is_some()

def absent(Optional[int64] value = None) -> bool:
    return value.is_none()

def pick(Optional[int64] value = None) -> int64:
    return value.unwrap_or(-1)

def present_str(Optional[str] value = None) -> bool:
    return value.is_some()

struct Adjuster:
    int64 base

    def resolve(self, Optional[int64] delta = None) -> int64:
        if delta.is_some():
            return self.base + delta.unwrap()
        return self.base

def main():
    # Omitted positional argument -> default `None` (the issue's minimal repro).
    print(present())  # False
    print(absent())  # True
    print(pick())  # -1
    print(present_str())  # False (pointer payload)

    # `None` passed explicitly at the call site.
    print(present(None))  # False
    print(pick(None))  # -1

    # A present value still works through the same path.
    print(present(Optional.Some(5)))  # True
    print(pick(Optional.Some(5)))  # 5

    # A typed `Optional[int64]` value holding None, passed in.
    Optional[int64] n = None
    print(present(n))  # False
    print(pick(n))  # -1

    # Method call: omitted argument fills the None default (the redis#9 trigger).
    Adjuster c = Adjuster(10)
    print(c.resolve())  # 10  (omitted -> default None)
    print(c.resolve(Optional.Some(5)))  # 15  (present)
    print(c.resolve(delta=None))  # 10  (explicit keyword None)
    print(c.resolve(n))  # 10  (typed Optional None value)
