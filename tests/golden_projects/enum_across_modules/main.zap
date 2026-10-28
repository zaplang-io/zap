# Cross-module enum: `shapes.Shape.Circle(...)` construction and a cross-module
# call `shapes.area(...)`.
import shapes

def main():
    print(shapes.area(shapes.Shape.Circle(r=2.0)))
