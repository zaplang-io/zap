# A library module whose public functions feed bare payloads into `Optional[T]`
# positions (#1265): a bare `return` payload and a bare `Optional[T]` argument.
# The widening must hold across the modlink path when these are called from an
# importing module, exactly as in the declaring module.
def boxed(int64 n) -> Optional[int64]:
    return n


def unwrap_or_zero(Optional[int64] x) -> int64:
    return x.unwrap_or(0)
