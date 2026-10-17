# Proposal: Perl-style regular expressions

- **Author(s):** Zap team
- **Last updated:** 2026-10-17
- **Status:** Accepted
- **Discussion:** (internal) — see `DECISIONS.md` D-0010

## Abstract

Zap provides first-class Perl-style regular-expression literals — `m/.../` for
matching and `s/.../.../` for substitution — bound to subjects with the `=~`
(matches) and `!~` (does not match) operators, plus a `~=` in-place substitution
shorthand. Captures are exposed through a returned **match object**, not magic
globals. The engine is PCRE2.

## Background

The target audience (Python developers) expects powerful, ergonomic regex, and
the design brief calls for Perl-5-like syntax. The prior design used only a
`=~` operator over string patterns; this proposal upgrades regex to first-class
literal syntax while avoiding two classic hazards: lexer ambiguity with
division, and concurrency-hostile magic match variables.

## Proposal

### Literals

- `m/PATTERN/FLAGS` — a match literal.
- `s/PATTERN/REPLACEMENT/FLAGS` — a substitution literal.
- Alternate delimiters are allowed to avoid escaping: `m{...}`, `m!...!`,
  `s{...}{...}`. The delimiter after `m`/`s` is the active delimiter.
- Flags: `g` (global), `i` (case-insensitive), `m` (multiline `^$`),
  `s` (dotall), `x` (extended/comments). Flags are a suffix.

### Operators

- `subject =~ regex` — apply the regex to the subject.
  - With `m//`: returns a **match object** (truthy) on success, `none` on no
    match. With the `g` flag, returns an iterable of match objects.
  - With `s//`: returns a **new string** (the subject is never mutated).
    With `g`, replaces all; otherwise the first occurrence.
- `subject !~ m//` — returns `true` when there is no match (boolean).
- `subject ~= s/a/b/g` — in-place substitution **shorthand**; desugars exactly
  to `subject = subject =~ s/a/b/g`. Requires `subject` to be an assignable
  lvalue.

### Lexing (ambiguity resolution)

A regex literal is recognized **only** after an `m` or `s` keyword-prefix. A
bare `/` is therefore **always** division — Zap does not use Perl's
term-vs-operator context heuristics. This keeps the lexer simple and
predictable.

### Captures — match objects, no globals

The result of a successful match is a match-object value the programmer binds to
a local name of their choosing. There are **no implicit capture variables**
(`$1`, `$~`).

```python
auto mt = text =~ m/(\d{4})-(\d{2})-(\d{2})/
if mt:
    print(mt.group(0))      # whole match
    print(mt.group(1))      # first group (positional)

# named groups:
auto d = line =~ m/(?<year>\d{4})-(?<month>\d{2})/
if d:
    print(d["year"], d.month)   # both index and attribute access

# inline-binding sugar for the common conditional case:
if line =~ m/(?<ip>\d+\.\d+\.\d+\.\d+)/ as d:
    handle(d.ip)

# global match -> iterable of match objects:
for mt in text =~ m/\w+/g:
    print(mt.group(0))
```

### Substitution examples

```python
auto cleaned = raw =~ s/\s+/ /g     # collapse whitespace -> new string
name ~= s/^\s+|\s+$//g              # trim in place (shorthand)
```

## Rationale

- **Why require `m`/`s` prefixes:** bare `/.../ ` regex forces the lexer to
  track whether `/` begins a term or an operator — a well-known source of
  parser complexity and surprising bugs. Mandating the prefix removes the
  ambiguity entirely for negligible ergonomic cost.
- **Why `s///` returns a copy:** immutable-result semantics are simpler for the
  GC and avoid aliasing surprises; the `~=` shorthand recovers one-line
  ergonomics for the common mutate-this-variable case.
- **Why a returned match object and no magic globals:** Zap uses an M:N
  goroutine scheduler (D-0008). A goroutine can migrate between OS threads, so a
  thread-local "last match" is unstable, and any shared match state invites data
  races. A returned value bound to a local is **coroutine-safe by
  construction** — there is simply no shared match state to coordinate. This is
  the direct answer to "how do coroutines handle the match variable": they do
  not need to.

## Compatibility

New syntax; no existing code to break. `m` and `s` become contextual keywords
when immediately followed by a delimiter; they remain usable as identifiers
elsewhere (lexer rule: `m`/`s` + delimiter char starts a literal).

## Implementation

Roadmap **Phase 3, Regex track**. Depends on the `str` type (Phase 2) and a
PCRE2 binding in the runtime. Steps:

1. Lexer: recognize `m<delim>...<delim><flags>` and
   `s<delim>...<delim>...<delim><flags>`.
2. AST: `RegexMatch`, `RegexSubst` literal nodes; `=~`, `!~`, `~=` operators.
3. Runtime: PCRE2 wrapper; `MatchObject` type (groups, named groups, spans).
4. Codegen: compile literal patterns once (cache compiled program); fall back
   to runtime compilation for dynamic patterns (future).
5. Desugar `~=` in the front-end before IR.

## Open issues

- `catch` vs `except` keyword (shared with exceptions; tracked in D-0007) — only
  matters here for examples.
- Whether to add a `re2"..."` linear-time literal later for untrusted input.
- Compile-time pattern validation for literal patterns (nice-to-have).
