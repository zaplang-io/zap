total = 0
for _ in range(2):
    for i in range(200000):
        s = "k" + str(i)
        total += len(s)
print(total)
