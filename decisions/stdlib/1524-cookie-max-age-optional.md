---
title: "Cookie.max_age is Optional[int64] (amends D-0231)"
status: Accepted
issue: 1524
pr:
date: 2026-11-25
domain: stdlib
supersedes:
superseded-by:
---

# `http.parse_set_cookie` — `Cookie.max_age` is `Optional[int64]` (#1524)

**Status:** Accepted — amends the cookie-primitives decision D-0231 (#989).

## Decision

The prelude `Cookie` value struct returned by `http.parse_set_cookie(value)`
now types its `max_age` field as `Optional[int64]` instead of a plain `int64`:

- **absent or malformed** `Max-Age` (attribute omitted, empty, a bare `-`, or
  non-numeric) → `None`;
- **well-formed** `Max-Age` → `Some(n)`, for every sign and magnitude,
  **including `Some(0)` and `Some(-1)`**.

Nothing else about the type changes; the other eight fields keep their shape and
index, and `parse_set_cookie` keeps its single `value` argument.

## Rationale

D-0231 gave `max_age` the C sentinel `-1` for an absent attribute. But `-1` is
also a perfectly valid wire value, and RFC 6265 §5.2.2 assigns it real meaning: a
non-positive `Max-Age` expires the cookie immediately and takes precedence over
any future `Expires`. The sentinel therefore conflated two states a conforming
client must distinguish — "no `Max-Age`, defer to `Expires`/session" versus
"delete now" — and made it impossible for `zaplang-io/requests` (requests#3) to
apply expiration/precedence without reparsing the raw header the core had
already parsed.

Options considered:

1. **`Optional[int64]` (chosen).** Now that `Optional[T]` is fully supported
   (#1370), this is the Python-/Rust-faithful "value may be absent" shape. `None`
   covers absent and malformed; `Some(n)` carries any parsed value, so `-1` and
   `0` are first-class. A caller writes `if ck.max_age is None` / `.unwrap()` /
   `.unwrap_or(d)` — no sentinel lore. Rejected alternatives lose to this on
   ergonomics and on matching the rest of the language's absence story.
2. **Separate `has_max_age: bool` flag beside the `int64`.** Works, but it is two
   fields a caller must keep in sync by hand, it is not how any other nullable
   result in the stdlib is shaped, and it still invites reading the `int64` when
   the flag is false. Rejected.
3. **A different out-of-band sentinel (e.g. `int64::MIN`).** Just moves the
   collision; every sentinel is a legal `Max-Age`. Rejected.

Malformed input folds into `None` (rather than raising) because a `Set-Cookie`
parser is lenient by design and the surrounding fields are still useful; a caller
that wants to reject garbage can, since absent and malformed are both simply
"no usable `Max-Age`".

## Consequences

- **Caller code** reads `max_age` as an Optional: `is None` / `is_some` /
  `unwrap` / `unwrap_or`. requests#3 can now apply RFC 6265 §5.2.2 (a valid
  non-positive `Max-Age` deletes even with a future `Expires`) directly off the
  typed result — `expires` is still exposed as the raw date string, so the caller
  has both and can let `Max-Age` win.
- **Runtime.** The C parser (`crates/codegen/src/runtime/crypto_jose.inc.c`)
  tracks a `max_age_present` flag beside the signed value instead of overloading
  `-1`; a new `zap_http_cookie_max_age_present` accessor exposes it, and lowering
  (`crates/ir/src/lower.rs`) wraps the value/flag pair into the Optional field
  (`Some` when present, `None` otherwise) — the same Optional-construction path
  the JSON-decode and scalar-wrap lowerings use.
- **Surface churn.** `http` is Experimental (see `docs/STABILITY.md`), so this is
  an additive/behavioural change requiring no stability promise. The only
  observable break for existing code is that `ck.max_age` is no longer an
  `int64`; a `-1` sentinel comparison must become an Optional check.
- `parse_set_cookie`'s signature is unchanged (one positional `value`), so
  `crates/syntax/src/seam_sigs.rs` and the `seam_kwargs` tests need no update —
  this is a field-type change on the result struct, not an argument change.
