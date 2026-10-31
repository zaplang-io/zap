# Golden fixture: reflection-driven handler adapter — `Depends(...)` injection
# (I#466, D-0092).
#
# The idiomatic FastAPI form: a handler declares a dependency as a parameter with
# a `Depends(provider)` default. Its type is `fn(Request, DB) -> Response`, which
# does NOT match the uniform `fn(Request) -> Response` seam. At the `@app.get`
# decoration site the compiler synthesizes a statically-typed adapter
#
#     def list_items$adapter(Request req) -> Response:
#         DB get_db$dep = get_db()
#         return list_items(req, get_db$dep)
#
# and registers THAT on the seam. `App.get` invokes the registered handler with a
# live Request to prove the dependency is resolved and flows through: `get_db()`
# returns DB(42), so the Response carries status 42 — produced by injection, not
# pulled off the request.

struct Request:
    str path

struct Response:
    int64 status

struct DB:
    int64 conn

def get_db() -> DB:
    return DB(42)

struct App:
    def get(self, str path, fn(Request) -> Response h):
        Request req = Request(path)
        fn(Request) -> Response f = h
        Response r = f(req)
        print(path + " -> " + str(r.status))

@app.get("/items")
def list_items(Request req, DB db = Depends(get_db)) -> Response:
    return Response(db.conn)

def main():
    auto app = App()
    print("ready")
