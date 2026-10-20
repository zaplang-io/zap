# Golden fixture: a struct field literally named `name` (I#141). Lowering used
# to reserve `.name` for the enum `.name` op (D-0018); the receiver type now
# decides, so a struct with a `name` field reads/writes it as a plain field.
struct Person:
    str name
    int64 age

struct Team:
    Person lead
    str name

def main():
    auto p = Person(name="Ada", age=36)
    print(p.name)           # Ada
    p.name = "Grace"
    print(p.name)           # Grace
    print(p.age)            # 36

    # Nested: a struct whose field is `name`, reached through another struct.
    auto t = Team(lead=Person(name="Linus", age=54), name="core")
    print(t.name)           # core
    print(t.lead.name)      # Linus
    t.lead.name = "Guido"
    print(t.lead.name)      # Guido
