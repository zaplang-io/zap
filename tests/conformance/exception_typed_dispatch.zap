# claim: EXC-5, EXC-7
# SPEC §10 (I#471): typed exceptions retain their identity, match subclasses,
# dispatch except clauses in source order, preserve the message binding, and
# propagate through finally when no clause matches.
struct ParseError:
    str message

def main():
    try:
        raise ParseError("bad syntax")
    except TypeError:
        print("wrong custom type")
    except ParseError as e:
        print("caught custom", e)

    try:
        raise ValueError("bad value")
    except TypeError:
        print("wrong type")
    except ValueError as e:
        print("caught", e)
    except Exception as e:
        print("wrong base type", e)
    finally:
        print("finally handled")
    print("after handled")

    list[int64] data = [1]
    try:
        auto value = data[3]
        print(value)
    except KeyError:
        print("wrong exact type")
    except LookupError:
        print("caught lookup parent")

    try:
        try:
            raise ValueError("propagate me")
        except TypeError:
            print("wrong nested type")
        finally:
            print("finally propagating")
    except Exception as e:
        print("caught propagated", e)

    try:
        try:
            raise ValueError("first")
        except ValueError:
            raise TypeError("replacement")
        finally:
            print("finally in handler")
    except TypeError as e:
        print("caught replacement", e)

    try:
        raise Cancelled("cancel")
    except Exception:
        print("wrong base class")
    except BaseException:
        print("caught base exception")
