# #1374: a tier-1 (all-unit) enum derive using `comptime v.name` and no payload
# binding — the comptime arm expands to one `case` per constant variant.
@derivable(Tag)
def _derive_tag[T](T value) -> str:
    match value:
        case comptime v in reflect.type_info(T).variants:
            return comptime v.name
    return ""

interface Tag:
    def tag(self) -> str

@derive(Tag)
enum Color:
    Red
    Green
    Blue

def main():
    print(Color.Red.tag())
    print(Color.Green.tag())
    print(Color.Blue.tag())
