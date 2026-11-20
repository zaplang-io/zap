# A library module exporting a struct whose field default is a generic
# constructor (#1152). The default must be monomorphized when the struct is
# constructed in an *importing* module (the modlink path), exactly as it is in
# the declaring module — this is the cross-module half of the regression.
struct Client:
    Atomic[int64] state = Atomic(0)
    Atomic[bool] closed = Atomic(False)
    int64 port = 11211
