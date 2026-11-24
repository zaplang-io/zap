---
title: Structured fields on package-defined exceptions
status: Locked
issue: 1432
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Structured fields on package-defined exceptions

**Status:** Locked

## Decision

A package-defined `@exception` struct (#1278) may carry its **declared fields**
across a `raise`/`except` boundary — of any type, including non-string and
`Optional` fields — and a typed catch reads them as fields, without parsing the
diagnostic message:

```zap
@exception
struct TokenEndpointError:
    str message
    str error
    int64 status
    Optional[str] uri = None

def main():
    try:
        raise TokenEndpointError("request refused", "invalid_client", 401)
    except TokenEndpointError as failure:
        assert failure.error == "invalid_client"
        assert failure.status == 401
```

**Compatibility model — the catch binding is unchanged; fields are additive.**
`except E as e` still binds `e` to the exception's **message string**, so
`str(e)`, `print(e)`, every #1278 message-only catch, and the uncaught
diagnostic are **identical** to before. The declared struct instance rides
alongside the message as a *payload* on the exception object, and `e.<field>`
reads a declared field off that payload. The two surfaces are orthogonal:
`e` is the message, `e.<field>` is a field.

- **Raise.** `raise E(args...)` constructs the real `E` instance with the
  ordinary constructor machinery — positional and keyword arguments, declared
  defaults, and the `@validate`/`@range` field checks — then derives the
  exception's message and attaches the instance as the payload. A single
  positional message (`raise E("msg")`) keeps working: it just sets the one
  field, so every #1278 exception raises exactly as before. A raise with extra
  declared fields, previously rejected with `ZAP6001 "E() takes one positional
  message"`, now succeeds.
- **Message.** The exception's message (what `str(e)` / the diagnostic shows) is
  the value of a declared `str message` field when present — the convention
  every #1278 exception already follows — else the struct's first `str` field,
  else the empty string.
- **Catch.** `except E as e` records `E`'s struct id against the caught
  exception, so `e.<field>` loads the payload and does an ordinary struct field
  read. A message-only / built-in / catch-all clause records nothing, leaving
  `e` the plain message string.

The typed fields `e` exposes are those **declared on the clause's static type**
`E`. For a **concrete** catch — every direct catch, including the OAuth token
endpoint use case — the payload is exactly an `E`, so the read is exact.

## Rationale

- **The catch binding stays the message.** Rebinding `e` to the struct instance
  would change what `print(e)` / `str(e)` mean for existing `@exception` structs
  (the #1278 fixtures print the message), i.e. it would not preserve message-only
  semantics literally. Keeping `e` the message and exposing fields through
  `e.<field>` preserves every existing catch unchanged while still giving typed
  field access — and it is Python-shaped (`str(e)` is the message, `e.attr` is a
  field).
- **Reuse, not a parallel path.** The raise reuses the struct constructor
  (`construct` → `fill_struct_fields`), so defaults, keyword arguments, and
  field validators apply automatically; the catch reuses ordinary struct
  `FieldGet`. The only new surface is a payload slot and two accessors.
- **`e.<field>` mirrors `e.cause` / `eg.exceptions`.** A caught exception already
  exposes pseudo-attributes read off the stashed exception object (#604, #605);
  a declared field read is the same shape, resolved against the recorded struct
  id.
- **The runtime already traces precisely.** `ZapException` is a precise GC
  object; adding the payload as a fourth traced pointer keeps the struct (and in
  turn its own pointer fields) live while the exception is in flight.

## Consequences

- **Additive, non-breaking.** The catch binding, message-only catches,
  catch-family dispatch (#1278), `str(e)`, and the uncaught diagnostic are
  unchanged. Code that only raised/caught message-only exceptions behaves
  identically; raising an `@exception` struct with extra fields, previously a
  compile error, now works.
- **Cross-module.** The payload struct id and the `except pkg.E` clause resolve
  to the same consistently-mangled symbol, so a member raised inside a package
  is caught — and its fields read — by the importer's typed catch, exactly as
  cross-module family dispatch already works.
- **No new syntax.** The constructor-call raise and field-access read already
  parse; `docs/GRAMMAR.ebnf` is unchanged. No new `ast`/IR variant is added —
  the lowering reuses `StructNew`/`FieldGet` and two runtime calls.
- **Field reads use the static clause type.** Under a family **base** catch
  (`except Base as e`) where the runtime exception is a different concrete
  member, `e` still binds the message soundly; reading a base-declared field is
  positional against the member payload, so it is well-defined only when members
  keep the base's fields as a layout prefix (the natural pattern — a specific
  error extending a general one). v1 does not enforce prefix compatibility; a
  base clause that must read member-specific fields should narrow to the
  concrete type first. Zap exception families are catch-only with no layout
  relationship (#1278), so this boundary is inherent, not introduced here.
- **Runtime.** `ZapException` gains a `payload` slot (a fourth precisely-traced
  pointer) and the accessors `zap_exception_set_payload` /
  `zap_exception_payload`. The built-in exception ids and their message-only
  path are untouched.

Unblocks the OAuth token-endpoint error payload (zaplang-io/oauth#3; RFC 6749
§5.2), which exposes a provider error code, optional description/URI, and HTTP
status as typed fields — matching Python Authlib and Go's `oauth2.RetrieveError`.
