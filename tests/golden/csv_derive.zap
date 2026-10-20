# Golden fixture: @derive(Csv) record codec (D-0059 §4b, I#132), reusing the
# shared derive-walk core from @derive(Json). A struct maps to one CSV record
# (fields = columns, declaration order). to_csv quotes a field only when needed
# (comma/quote/newline); from_csv splits a record and parses each column to its
# field's scalar type. Booleans use Python's csv convention (True/False).

@derive(Csv)
struct Row:
    str city
    int64 pop
    bool capital
    float64 lat

def main():
    # Encode: the city contains a comma, so it is quoted.
    auto r = Row(city="A,B", pop=100, capital=True, lat=1.5)
    print(r.to_csv())                       # "A,B",100,True,1.5

    # Decode then re-encode (round-trip): a plain unquoted record.
    auto r2 = Row.from_csv("Paris,42,true,48.8")
    print(r2.to_csv())                      # Paris,42,True,48.8
