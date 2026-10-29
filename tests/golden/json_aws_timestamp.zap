# Golden fixture: @derive(Json) AWS timestamp hook (ticket #346, D-0086). A
# DateTime field (de)serializes as epoch seconds (the default, a JSON number) or
# ISO-8601 (a JSON string) selected per field with @json(timestamp=...). Driven
# from a FIXED Unix timestamp so output is deterministic/UTC: 1700000000 =
# 2023-11-14T22:13:20Z.
@derive(Json)
struct Event:
    str name
    DateTime at
    @json(timestamp="iso8601")
    DateTime seen

def main():
    auto dt = datetime.from_timestamp(1700000000)
    auto e = Event(name="boot", at=dt, seen=dt)
    print(e.to_json())
    # {"name":"boot","at":1700000000,"seen":"2023-11-14T22:13:20"}

    auto back = Event.from_json(
        "{\"name\":\"boot\",\"at\":1700000000,\"seen\":\"2023-11-14T22:13:20\"}",
    )
    print(back.name)  # boot
    print(datetime.to_timestamp(back.at))  # 1700000000
    print(datetime.to_timestamp(back.seen))  # 1700000000
