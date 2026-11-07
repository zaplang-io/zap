# diff: diverge BUG #781: `\x` / `\u` string escapes are not decoded
# Intent (CPython parity, epic #374 walled garden): `"\x41"` is "A" and
# `"café"` is "café". Zap's `unescape` (crates/ir/src/lower.rs ~18391)
# decodes only `\n \t \r \0 \\ \' \"`; `\x`/`\u`/`\U`/`\N{}` fall through and are
# kept verbatim WITH the backslash, so these print `\x41` and `café`
# literally. Pinned as a documented divergence; when #781 lands the harness flags
# DIVERGENCE-GONE — promote it to `# diff: match` then.
def main():
    print("\x41")
    print("café")
