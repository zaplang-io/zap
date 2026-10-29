# Golden fixture: @derive(Json) AWS open-enum hook (ticket #346, D-0086). An
# enum marked @json(open) (de)serializes as a plain JSON string: a known
# (payload-free) variant maps to its name; a string the enum does not name
# decodes to the Unknown(str) fallback variant (forward-compatible with AWS
# adding enum values), and encodes back to the carried string. Also exercised
# nested as a struct field.
@derive(Json)
@json(open)
enum Flavor:
    Vanilla
    Chocolate
    Unknown(str value)

@derive(Json)
struct Scoop:
    Flavor flavor
    int64 count

def main():
    auto van = Flavor.Vanilla
    print(van.to_json())  # "Vanilla"

    auto other = Flavor.Unknown(value="Strawberry")
    print(other.to_json())  # "Strawberry"

    # A known string decodes to its variant; an unknown one to Unknown(raw).
    print(Flavor.from_json("\"Chocolate\"").to_json())  # "Chocolate"
    print(Flavor.from_json("\"Mango\"").to_json())  # "Mango"

    # Nested as a struct field: unknown value survives the round trip.
    auto s = Scoop(flavor=Flavor.from_json("\"Mango\""), count=2)
    print(s.to_json())  # {"flavor":"Mango","count":2}

    auto back = Scoop.from_json("{\"flavor\":\"Vanilla\",\"count\":5}")
    print(back.to_json())  # {"flavor":"Vanilla","count":5}
