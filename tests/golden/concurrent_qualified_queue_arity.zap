# I#996, D-0202 (negative): the dotted constructor is sugar for the bare
# `new_queue[T]()`, which takes NO value arguments — so passing one is a compile
# error, diagnosed exactly as the bare form would be. Locks in that the sugar
# never widens the constructor's arity.
import concurrent

def main():
    concurrent.Queue[int64] q = concurrent.Queue[int64](5)
    q.push(1)
