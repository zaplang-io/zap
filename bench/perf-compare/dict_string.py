d = {}
for i in range(200000):
    d["k" + str(i)] = i
s = 0
for i in range(200000):
    s += d["k" + str(i)]
print(s)
