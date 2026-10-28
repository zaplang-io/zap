# Cross-module struct use: the dotted type annotation `geo.Point`, the imported
# constructor helper, a cross-module function call, and field access.
import geo

def main():
    geo.Point p = geo.make(3, 4)
    print(geo.manhattan(p))
    print(p.x)
