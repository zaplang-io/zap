# shapes

Shape utilities.

A tiny module used to exercise ``zap doc`` rendering. It covers a
constant, functions, a struct with fields and methods, an enum, and an
interface — the full item surface.

## Contents

- **Functions**
  - [area](#shapes-area)
  - [clamp](#shapes-clamp)
- **Types**
  - [Point](#shapes-point)
- **Enums**
  - [Shape](#shapes-shape)
- **Interfaces**
  - [Drawable](#shapes-drawable)
- **Constants**
  - [PI](#shapes-pi)

## Functions

<a id="shapes-area"></a>

### area

```zap
@memoize
def area(float64 r) -> float64
```

Area of a circle.

**Args:**
    r: the radius, in metres

**Returns:**
    the area enclosed by the circle

<a id="shapes-clamp"></a>

### clamp

```zap
def clamp[T](T x, T lo, T hi) -> T
```

*where T: Ordered*

Clamp ``x`` into the inclusive range ``[lo, hi]``.

## Types

<a id="shapes-point"></a>

### Point

```zap
struct Point
```

**Fields**

| Name | Type | Default |
|------|------|---------|
| x | `float64` |  |
| y | `float64` |  |

**Methods**

<a id="shapes-point-dist"></a>

#### dist

```zap
def dist(Point other) -> float64
```

Euclidean distance to `other`.

## Enums

<a id="shapes-shape"></a>

### Shape

```zap
enum Shape
```

**Variants**

- `Circle(float64 r)`
- `Square(float64 side)`

**Methods**

<a id="shapes-shape-describe"></a>

#### describe

```zap
def describe(self) -> str
```

Human label for the shape.

## Interfaces

<a id="shapes-drawable"></a>

### Drawable

```zap
interface Drawable
```

**Methods**

<a id="shapes-drawable-draw"></a>

#### draw

```zap
def draw(self) -> str
```

## Constants

<a id="shapes-pi"></a>

### PI

```zap
const float64 PI
```
