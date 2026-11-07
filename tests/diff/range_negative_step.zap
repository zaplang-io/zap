# diff: match
# A negative-step range counts down, matching CPython (#374, fixed by #759):
# `range(5, 0, -1)` is 5,4,3,2,1 (sum 15) and `range(10, 0, -2)` is 10,8,6,4,2
# (sum 30). A range whose direction contradicts its bounds is empty
# (`range(0, 5, -1)` yields nothing, sum 0).
def main():
    auto a = 0
    for i in range(5, 0, -1):
        a = a + i
    print(a)
    auto b = 0
    for i in range(10, 0, -2):
        b = b + i
    print(b)
    auto c = 0
    for i in range(0, 5, -1):
        c = c + i
    print(c)
