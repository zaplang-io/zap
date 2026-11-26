# Golden fixture: str.ljust/rjust/center repeat the FULL fill codepoint, so a
# multibyte fill yields valid UTF-8 (#1648, superseding D-0210's ASCII-fill
# byte). Width counts codepoints; the allocation counts bytes. A fill that is
# not exactly one codepoint raises a catchable TypeError (CPython). Follow-up to
# the Unicode string audit, epic #1638.
#
# center's odd-padding split now matches CPython: the extra fill goes on the LEFT
# when both the width and the margin are odd (#1656, superseding D-0210).
def main():
    # -- 2-byte fill (é) ------------------------------------------------------
    print("x".ljust(3, "é"))  # xéé
    print("x".rjust(3, "é"))  # ééx
    print("x".center(3, "é"))  # éxé (even pad)
    print(len("x".ljust(3, "é")))  # 3
    print(len("x".center(3, "é")))  # 3

    # -- 3-byte fill (€, U+20AC) ----------------------------------------------
    print("x".ljust(3, "€"))  # x€€
    print("x".rjust(3, "€"))  # €€x
    print(len("x".rjust(3, "€")))  # 3

    # -- 4-byte fill (😀, U+1F600) --------------------------------------------
    print("x".ljust(3, "😀"))  # x😀😀
    print(len("x".ljust(3, "😀")))  # 3

    # -- multibyte subject + multibyte fill -----------------------------------
    print("é".center(5, "α"))  # ααéαα (even pad 4)
    print(len("é".center(5, "α")))  # 5
    print("αβ".ljust(4, "γ"))  # αβγγ
    print("αβ".rjust(4, "γ"))  # γγαβ

    # -- odd centered padding (CPython split: extra fill on the left) ---------
    print("x".center(4, "é"))  # éxéé (width 4 even: left 1, right 2)
    print(len("x".center(4, "é")))  # 4
    print("ab".center(5, "é"))  # ééabé (width & margin odd: left 2, right 1)

    # -- default (space) and explicit ASCII fill are unchanged ----------------
    print(f"[{'hi'.ljust(5)}]")  # [hi   ]
    print("hi".rjust(5, "*"))  # ***hi
    print("hi".center(6, "-"))  # --hi--

    # -- width at or below the subject's codepoint length -> unchanged --------
    print("héllo".ljust(3, "x"))  # héllo (already wider)
    print("héllo".center(2, "x"))  # héllo
    print(len("héllo".ljust(1, "é")))  # 5 (valid fill, no padding)

    # -- empty subject --------------------------------------------------------
    print("".ljust(3, "é"))  # ééé
    print(len("".center(4, "€")))  # 4

    # -- a fill that is not exactly one codepoint raises catchable TypeError ---
    try:
        print("x".ljust(3, "ab"))
    except TypeError:
        print("multi-char-fill-caught")
    try:
        print("x".center(3, ""))
    except TypeError:
        print("empty-fill-caught")
    try:
        print("x".rjust(3, "éé"))
    except TypeError:
        print("multi-cp-fill-caught")
    # the fill is validated even when the width needs no padding (CPython)
    try:
        print("héllo".ljust(2, "ab"))
    except TypeError:
        print("short-width-still-validates")
