# A sibling module imported by main (I#283/D-0081). Its public `helper` is
# reachable as `a.helper`; `_hidden` is private (leading underscore) and cannot
# be accessed through the import.
def helper() -> int64:
    return 7

def _hidden() -> int64:
    return 99
