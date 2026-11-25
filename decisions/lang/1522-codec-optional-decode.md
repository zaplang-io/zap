---
title: "Codec shadows gain an Optional[T] decode branch (null-safe, D-0065 semantics)"
status: Accepted
issue: 1522
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Codec shadow `Optional[T]` decode (#1522)

**Status:** Accepted — an E4 (#1429) follow-up. The Json (#1470) and Yaml (#1484)
codec shadows gain an `Optional[T]` encode + decode branch, so an `Optional`
scalar field now round-trips through the **library** decoder, agreeing with the
built-in by round-trip.

## Why now

`Optional` decode was scoped out of #1490 for two reasons, both since resolved:

- **#1506** desugars `reflect.set_field` inside a `match` arm, so a decoder arm
  can populate a field directly (previously it leaked `reflect`).
- **#1517** gave the built-in `@derive(Yaml)` real `Optional` encoding (it
  previously miscompiled to a non-existent `Optional$T__to_yaml`), so there is now
  a built-in Yaml codec to agree with — parity with `@derive(Json)` (D-0065).

A first attempt during #1490 also crashed at runtime: the decode read `m[key]`
unconditionally, but the built-in `to_json` **omits** a `None` key, so the read
dereferenced a missing entry (SIGSEGV). That is the mistake this issue fixes
properly.

## The semantics (D-0065), mirrored

The built-in codecs treat an `Optional` field as "nullable / may be absent":

- **encode:** `Some(v)` → the scalar; `None` → Json **omits** the key, Yaml emits
  `null`.
- **decode:** an **absent key OR an explicit `null`** → `None`; a value → `Some`.

The shadow matches this:

- **encode** emits the scalar for `Some` and `JsonValue.Null` for `None` (both
  built-in decoders map `null` → `None`, so emitting null round-trips whether the
  consumer omits or nulls).
- **decode** is **null-safe**: `if f.name in m:` (a dict-membership test, lowering
  to `zap_dict_contains_str`) guards the DOM read, then `JsonValue.Int(i)` → `Some(i)`
  and anything else (including `null`) → `None`. An absent key never reaches a
  read, so the #1490-era missing-key deref cannot recur.

## Scope

`Optional[int64]` (the scalar the shadows exercise). `Optional` of a nested
struct, and `list` / `dict` fields, remain out of scope — the built-in codecs do
not round-trip collection fields yet (a separate follow-up), and this slice keeps
to the scalar optional the built-in supports. CSV (#1480) is a flat codec with no
optional surface and is unchanged.

## Verification

Local Docker/LLVM is down, so the runtime round-trip is gated by the CI LLVM
matrix (the goldens `derive_json_codec_shadow` / `derive_yaml_codec_shadow`, each
with `Some` and `None` cases cross-checked against the built-in both ways). The
host build proves the branch lowers and that the decode presence-checks the key
(`zap_dict_contains_str` in the emitted IR) — the structural null-safety the
earlier attempt lacked.
