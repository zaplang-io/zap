# #637: destination types must reach empty literals at append/call/return sites.
def make() -> list[dict[str, bool]]:
    return [{}]

def count(dict[str, bool] value) -> int64:
    return len(value)

def take(list[str] items, dict[str, bool] fields):
    assert len(items) == 0
    assert len(fields) == 0

def main():
    list[bool] flags = []
    flags.append(False)
    flags.append(True)
    assert not flags[0]
    assert flags[1]
    list[dict[str, bool]] names = []
    names.append({})
    names.append({})
    names[0]["ready"] = True
    assert names[0]["ready"]
    assert len(names[1]) == 0
    names.append({"ready": False})
    assert not names[2]["ready"]
    list[list[str]] groups = []
    groups.append([])
    groups[0].append("alpha")
    assert groups[0][0] == "alpha"
    list[list[dict[str, bool]]] matrix = []
    matrix.append([{}])
    matrix[0][0]["enabled"] = True
    assert matrix[0][0]["enabled"]
    list[dict[str, dict[str, bool]]] trees = []
    trees.append({"branch": {}})
    trees[0]["branch"]["leaf"] = True
    assert trees[0]["branch"]["leaf"]
    # Context from earlier entries also applies to later empty nested literals.
    auto inferred = [{"known": True}, {}]
    inferred[1]["new"] = False
    assert not inferred[1]["new"]
    auto returned = make()
    returned[0]["returned"] = True
    assert returned[0]["returned"]
    assert count({}) == 0
    take([*[]], {**{}})
    matrix.append([*[{}]])
    assert len(matrix[1][0]) == 0
    trees.append({**{"branch": {}}})
    assert len(trees[1]["branch"]) == 0
    list[list[float64]] numbers = []
    numbers.append([])
    numbers[0].append(1.5)
    assert numbers[0][0] == 1.5
    print("nested empty append: PASS")
