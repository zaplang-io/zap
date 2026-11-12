d = {}
for i in range(200000):
    d[i] = i
s = 0
for i in range(200000):
    s += d[i]
print(s)
