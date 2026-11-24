# Golden fixture (#1395): a zero or negative duration fires effectively
# immediately — the absolute deadline is already in the past, so the firing
# coroutine's wait returns at once and delivers its value without blocking. Both
# receives complete and the program prints in order.
import time

def main():
    auto z = <-time.after(time.millis(0))
    print("zero fired")
    auto n = <-time.after(time.nanos(-5))
    print("negative fired")
