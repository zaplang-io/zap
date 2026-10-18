# Golden fixture: dict — str->int, int->int, and str->str. Exercises literal
# construction, d[k] get, d[k] = v set (insert + overwrite), and len(d).
def main():
    auto ages = {"ada": 36, "alan": 41}
    print(len(ages))
    print(ages["ada"])
    print(ages["alan"])
    ages["grace"] = 45
    ages["ada"] = 37
    print(ages["grace"])
    print(ages["ada"])
    print(len(ages))

    auto sq = {1: 1, 2: 4, 3: 9}
    print(sq[2])
    print(sq[3])
    print(len(sq))

    auto caps = {"fr": "paris", "jp": "tokyo"}
    print(caps["jp"])
