# A module-level mutable binding is not supported yet (#658 Phase 2): it is
# rejected with ZAP3018, pointing at the planned mutable-global surface.
int64 counter = 0

def main():
    print(counter)
