# Borrowed-bytes strview (#1082, discussion #1097): a non-owning view over an
# existing string's bytes — zero alloc, zero copy — for read-only consumers. The
# view is created explicitly with .view()/.view(lo,hi), sub-viewed, probed,
# compared, and materialized with .to_str(). Bounds are codepoint indices
# (D-0025), so a view over multibyte text counts codepoints, not bytes.
def main():
    str s = "hello, world"
    strview v = s.view(0, 5)  # "hello"
    print(len(v))  # 5
    print(v.startswith("he"))  # True
    print(v.endswith("lo"))  # True
    print(v.contains("ell"))  # True
    print(v == "hello")  # True
    print(v == "world")  # False
    strview w = s.view(7)  # "world" (to end)
    print(w == "world")  # True
    strview sub = v.view(1, 3)  # "el" (sub-view of a view)
    print(sub == "el")  # True
    print(w.to_str())  # world (materialized owned copy)
    print(v.startswith(s.view(0, 2)))  # True (a strview argument too)

    # Multibyte: bounds are codepoints, so a 2-codepoint view of "héllo" spans
    # the 2-byte 'é' correctly.
    str u = "héllo"
    strview uv = u.view(0, 2)  # "hé"
    print(len(uv))  # 2
    print(uv == "hé")  # True
