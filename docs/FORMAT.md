# The canonical Zap format (`zap fmt`, D-0037)

Zap has **one** canonical source style, enforced by `zap fmt`. There are no
options. A single style means every coding agent emits byte-identical
formatting — no style drift between runs, minimal diffs, and no style debate in
review. That is the point of the tool (pillars 1 and 2 of the Zap thesis:
written by agents, read by humans).

`zap fmt` reuses the parser and the comment trivia from the frontend (#225); it
parses the source to an AST, then prints that AST back in canonical form. It
**refuses to format source that does not parse** (it never rewrites code whose
meaning it cannot establish).

Two properties hold by construction and are enforced by the test suite:

- **Idempotence:** `fmt(fmt(x)) == fmt(x)`. Every rule is a fixed point and every
  wrapping decision is a pure function of the AST, not of the incoming layout.
- **Semantics- and comment-preservation:** the formatted output parses to the
  same AST (spans aside) and keeps every `#` comment.

## The rules

### Indentation
Four spaces per level (D-0041). Tabs are a lex error, so they never reach fmt.

### Line width
Target **88** columns (`zapc_fmt::MAX_WIDTH`). A construct that fits is written on
one line; one that does not is wrapped (below). A line that cannot be wrapped —
for example a single long string literal — is left over-length rather than
broken unsafely.

### Blank lines
Runs of blank lines are collapsed:

- **at most 2** between top-level items;
- **at most 1** inside any block (function/method body, control-flow block) and
  between struct/enum/annotation members;
- none at the very start of a block.

Author blank lines within those caps are preserved (0 stays 0).

### Strings
Prefer **double quotes** (`"`). Fall back to single quotes when the body contains
an unescaped `"` and no `'` (avoids needless escaping). If both appear unescaped,
keep `"` and backslash-escape the inner `"`. A string whose body contains a
literal newline is written triple-quoted (`"""…"""`). Prefixes are lowercased and
ordered `r`, `b`, `f` (so `rb`, `rf`). Raw-string bodies are never re-escaped.

### Spacing
One space around binary operators and after commas; `key: value` in dicts and
type annotations; no space inside brackets or before a call's `(`; no space
around `=` in keyword arguments and call-site defaults (`f(x=1)`); a space around
`=` in declarations and parameter defaults with a type (`int64 n = 5`).

### Trailing commas
A collection, call, or signature that is wrapped across multiple lines gets a
**trailing comma** on its last element (this keeps subsequent one-line diffs
minimal and is what re-triggers the wrap on the next run). A construct that fits
on one line has **no** trailing comma. (A Black-style *magic trailing comma* — an
author comma forcing an otherwise-fitting construct to explode — is **not** part
of this slice; wrapping is width-triggered. A comment inside a collection does
force it to wrap, so the comment keeps its place.)

### Wrapping
When a call, list, set, tuple, or dict does not fit, it explodes to one element
per line, indented one level, with a trailing comma, and the closing bracket on
its own line at the original indentation. Elements that still do not fit are
wrapped recursively.

### Imports
Each imported module is rendered on its **own line** (`import a, b` and the
grouped `import (a, b)` form both normalize to one `import` per line). Import
order is **not** rewritten in this slice — reordering is deferred to avoid any
risk to comment attachment and is a documented follow-up.

### Comments
Every `#` comment is preserved. A standalone (own-line) comment stays on its own
line at the surrounding indentation; a trailing comment is glued to the end of
its line with two spaces before the `#`. A single space is inserted after the
leading `#` unless the comment is empty or begins with `!` or `:` (shebang /
typed-comment conventions). Comment *column alignment* is not preserved (the
canonical form is "two spaces then `#`").

## Usage

```
zap fmt [paths...]      # rewrite files in place (or format stdin to stdout)
zap fmt --check [paths] # exit non-zero and list files that are not formatted
zap fmt -               # read stdin, write formatted source to stdout
```

`--check` writes nothing and is the mode CI and the contributor fmt-gate use.
