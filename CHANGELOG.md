# Changelog

All notable changes to the **Zap toolchain** (compiler, runtime, bundled
standard library, CLI, and package tooling — one artifact, one version) are
recorded in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project versions by [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html)
as defined in [`docs/STABILITY.md`](docs/STABILITY.md) (D-0070). How entries are
written and how a release is cut is described in
[`docs/RELEASING.md`](docs/RELEASING.md).

> **Pre-1.0.** Zap is in the `0.x` regime: **no compatibility is promised** and
> any release may change anything (STABILITY §2). The `1.0.0` promise and the
> frozen Stable set (D-0076) take effect only when 1.0 is actually cut; the
> version stays editorial until then. No release has been tagged yet, so
> everything built to date sits under **[Unreleased]** and will roll into the
> first tagged version when it is cut (see `docs/RELEASING.md`).

## [Unreleased]

The initial pre-1.0 toolchain. Summarised by surface rather than listed
commit-by-commit; the authoritative per-feature record is `DECISIONS.md`
(D-0001…D-0079) and the design docs under `design/`.

### Added

- **Value structs and tuples auto-derive `Equatable` + `Hashable`** (#808, epic
  #812, D-0139). A `value struct` (and a tuple) now auto-derives equality and
  hashing with **no `@derive`**, so it is usable as a `dict` key / `set` member and
  with `==`/`!=` out of the box — because a value struct is copied-on-insert and
  has no identity (D-0023/D-0051), by-value equality is its only coherent meaning.
  Synthesis is **conditional**: the pair is derived iff **every field is itself
  hashable** (recursively through nested value structs, tuples, and
  `Optional`s); a non-hashable field (e.g. a `list`) simply leaves the aggregate
  non-hashable, reported at key use with a message **naming the offending field**,
  never silently. Float fields inherit the #803 key semantics (`-0.0`
  canonicalized, `NaN` rejected at runtime). Override is **all-or-nothing**:
  hand-writing or `@derive`-ing either `__eq__` or `__hash__` suppresses the
  synthesized pair (and the existing Eq-without-Hash guard still applies).
  `Comparable`/`Ord` stays explicit, and reference structs keep identity equality
  unless they opt in. As a side effect a **float is now a hashable element** of a
  tuple or `Optional` key too (`dict[(int64, float64), V]`, `Optional[float64]`
  keys), consistent with bare `float64` keys (#800/#803).
- **Interface refinement `interface X: Y`** (#806, epic #812, D-0139). An
  interface may now **refine** one or more supertypes — `interface Hashable:
  Equatable`, `interface C: A, B` — written between the body `:` and the header
  newline. A type conforming to a refined interface must conform to every
  supertype transitively, and a generic bound `[T: Hashable]` makes the
  supertypes' methods (here `Equatable`'s `__eq__`) callable on `T` alongside the
  interface's own. The compiler-known **capability interfaces** `Equatable`,
  `Hashable`, and `Comparable` are defined with `Hashable: Equatable` and
  `Comparable: Equatable` (definitions only — they are not yet wired into the
  collection element guards, a later epic phase). Conformance failures name the
  missing supertype (so "satisfies `Hashable` but not `Equatable`" points at
  `Equatable`); refining a non-interface (`ZAP4081`), a refinement cycle
  (`ZAP4082`), and a duplicate supertype (`ZAP4083`) are rejected at the
  definition site. No change to existing single-bound behaviour for non-refined
  interfaces.
- **Capability/bounds model for collection elements — design locked** (#805, epic
  #812, D-0139; RFC #801). Records the authoritative decision (no behavior change
  yet) that a collection's element requirement — `dict`/`set` key = `Hashable`,
  `heap` element = `Comparable`, every member storing = `Equatable` — is a **bound
  on the existing D-0047 structural interfaces**, retiring the separate enumerated
  `ZAP4024`/`ZAP4027` element guards in later phases. Fixes the capability
  vocabulary as the adjectival family **`Equatable` / `Hashable` / `Comparable`**
  (with `Hashable` and `Comparable` refining `Equatable`), settling `Comparable`
  as the single total-order name (retiring "Orderable"); rules `Storable`
  compiler-internal (so `f32`/`bigint`/`complex` stay honest "not-yet-lowered"
  gaps, not type laws); locks conditional auto-derivation of `Equatable`+`Hashable`
  for value structs and tuples (explicit for reference structs and `Comparable`);
  and keeps the scope wall at container-element typing (no `Numeric`). Implementing
  PRs follow in the epic (#806 interface refinement first).
- **`float64` dict keys and set members** (#803 / #800 Phase 2, D-0138): `float`/
  `float64` is now a first-class **dict key** (`dict[float64, V]`) and **`set`
  member** (`set[float64]`), completing #800. A float key/member is canonicalised
  `-0.0 → +0.0` (so `0.0` and `-0.0` collide to one slot — Python `==` parity) and
  then hashed/compared as its IEEE bits; `NaN` is **rejected at runtime** on every
  key operation — insert, lookup, `in`, and `del` each raise a **catchable
  `ValueError`** (`"NaN cannot be used as a dict key"` / `"... as a set member"`),
  never a silent slot or an abort. With #800 Phase 1 this makes `float64` usable
  in every collection position; only `f32` collection elements stay deferred (#49)
  and bigint/complex stay rejected (`ZAP4024`).
- **`float64` collection values** (#800 Phase 1, D-0137): `float`/`float64` is now
  a first-class element in the collection *value* positions that need no hashing —
  a **dict value** (`dict[str, float64]`, `dict[int64, float64]`), a **heap**
  element (`heap[float64]`, min/max), and a **channel** element
  (`channel[float64]`) — extending `list[float]` (#569) to the remaining
  containers. Each float reuses the uniform 8-byte collection word via a bit
  reinterpret and is not GC-traced. A `heap[float64]` orders by a documented total
  order where `NaN` sorts greatest. (Float dict **keys** and **set** members
  followed in #800 Phase 2 — see above; `f32` collection elements stay deferred,
  #49.)
- **`crypto` private-key PEM builders** (#778, D-0136): `rsa_private_key_pem(n,
  e, d, p, q, dp, dq, qi)`, `p256_private_key_pem(d)`, and
  `ed25519_private_key_pem(d)` build an unencrypted PKCS#8 `BEGIN PRIVATE KEY`
  PEM from private JWK components — the symmetric counterpart to the public-key
  builders (#643). OpenSSL does all ASN.1/validation (RSA via `RSA_check_key`;
  P-256 derives the public point and runs `EC_KEY_check_key`; Ed25519 from the
  32-byte seed); malformed/inconsistent components raise a catchable
  `ValueError`. Unblocks private-JWK signing in the `jwt` library.
- **`crypto` AEAD symmetric encryption** (#772, D-0135): `aes256gcm_encrypt`/
  `aes256gcm_decrypt` and `chacha20poly1305_encrypt`/`chacha20poly1305_decrypt`
  — `(key, nonce, data, aad="") -> bytes`, over OpenSSL EVP, mirroring Python's
  `cryptography` `AESGCM`/`ChaCha20Poly1305`. Key is 32 bytes, nonce 12, and the
  16-byte tag is appended (`ciphertext || tag`). Decrypt verifies the tag and
  raises a catchable `ValueError` on any tamper/length error — it never returns
  unauthenticated plaintext. Nonce uniqueness is the caller's responsibility
  (use `crypto.random_bytes(12)`). Pinned by the NIST GCM and RFC 8439 §2.8.2
  KATs.
- **`crypto.scrypt`** (#775, D-0134): `crypto.scrypt(password, salt, n, r, p,
  dklen=64) -> bytes`, the scrypt password-based KDF (OpenSSL `EVP_PBE_scrypt`),
  mirroring Python `hashlib.scrypt`. `n` must be a power of two greater than 1
  and `r`/`p`/`dklen` positive; a bad parameter or a derivation failure raises a
  catchable `ValueError`. Pinned by the RFC 7914 §12 KATs. Closes the epic #761
  gap review (D-0134): **Argon2** stays deferred (not in OpenSSL's stable API
  across target versions, would add a dependency); symmetric **AEAD** (#772) is
  accepted for a later `M`; **X.509/TLS** (#773) and **public-key encryption /
  raw ECDH** (#774) stay out of `crypto`.
- **Unsigned integer types** (#552, D-0019 amendment). `uint8`/`uint16`/
  `uint32`/`uint64` (with short `u8`/`u16`/`u32`/`u64` spellings) and the
  pointer-width `usize` are now first-class scalar types — the prerequisite for
  the C FFI epic (#539, `@repr(c)` #542/#544), whose ABIs are saturated with
  `unsigned`/`size_t`, and for `uint8` array data. Overflow wraps (two's
  complement, matching C); they print as unsigned decimal (a `uint64` above
  `INT64_MAX` shows its true magnitude). There is **no implicit conversion
  between signed and unsigned** — a mixed arithmetic/comparison/bitwise
  operation is the new `ZAP4015` error; convert one side explicitly (`uint64(x)`
  / `int64(x)`). Implicit widening is allowed only up the fixed-width unsigned
  tower (`uint8`→`uint64`); `usize` converts only explicitly (its width is
  platform-dependent). `>>` on an unsigned value is a logical shift; `//`/`%`
  are unsigned `udiv`/`urem`; `/` is true (float) division as for signed. There
  is no unsigned literal suffix — `uintN(x)`/`usize(x)` conversions produce them,
  wrapping on out-of-range input (`uint8(-1) == 255`). `usize` is the canonical
  pointer-width name; `size_t` is only the C mapping target at the FFI boundary,
  never a Zap alias (D-0019 locks "no type aliases"). **Reserves the new type
  keywords** `uint8`/`uint16`/`uint32`/`uint64`/`u8`/`u16`/`u32`/`u64`/`usize`.
  An unsigned subscript index zero-extends and is never read as a from-the-end
  index: a small width is a plain offset, and a `uint64`/`usize` above
  `INT64_MAX` (e.g. `uintN(-1)`) raises a catchable `IndexError` rather than
  silently wrapping to `xs[-1]`, even though its bits match a signed `-1`.
- **Language correctness hardening** (epic #374, from CPython differential
  testing). Faults that were silently wrong, uncatchable, or undefined are now
  defined and catchable: `assert` raises a catchable `AssertionError` (so
  `except AssertionError` resolves); `float(s)` on a malformed string raises a
  catchable `ValueError` like `int(s)` (#741); integer `**` with a negative
  exponent raises `ValueError` instead of silently flooring to `0`; `INT64_MIN
  // -1` and `% -1` are defined wrapping-int64 results instead of a process
  abort (#742); float `/` by zero raises `ZeroDivisionError` instead of IEEE
  `inf`; and a negative shift count raises `ValueError` while a count `>= 64` is
  a defined "shifted out" value instead of LLVM poison (#740). `str`/`print`/
  f-string/JSON render a float as its shortest round-trip decimal (CPython
  `repr`) instead of 6-significant-digit `%g` (B1). `x == None` / `x != None` are
  consistent with `is None` instead of failing in lowering (B3). `len(str)` is
  the Unicode codepoint count, consistent with codepoint indexing/slicing/
  iteration, rather than the UTF-8 byte length (B6, D-0025). Augmented
  assignment to a subscript or field target (`xs[i] += v`, `d[k] += v`,
  `obj.f += v`) now compiles to an evaluate-once read-modify-write of that one
  slot — the base and a side-effecting index are evaluated exactly once, like
  CPython — instead of a `codegen error: only simple variable aug-assignment is
  supported` (#750). Float `%` is now floored (it takes the sign of the divisor,
  consistent with floored float `//` and integer `%`) instead of C's truncated
  `fmod`/`frem`, so mixed-sign float modulo matches CPython (`5.5 % -2` is
  `-0.5`) and `x % 0.0` raises a catchable `ZeroDivisionError` (#751). A
  three-argument `range(a, b, step)` now honors the sign of the step — a negative
  step counts down (`range(5, 0, -1)` is `5,4,3,2,1`) — and a zero step raises a
  catchable `ValueError` instead of looping forever; the loop condition used to
  be an unconditional `cur < stop` (#759). The conditional expression `a if cond
  else b` now evaluates **only the chosen branch** — it lowers to a real branch
  rather than an eager `select` of both operands — so the non-taken branch's side
  effects and faults never happen (`xs[0] if xs else default` returns the default
  on an empty list instead of raising) (#776). String literals now decode the
  hex and Unicode escapes `\xHH`, `\uHHHH`, and `\UHHHHHHHH` (previously kept
  verbatim with the backslash) — in a `str` they denote a Unicode codepoint
  (UTF-8), in a `bytes` literal `\xHH` is the raw byte; a surrogate or
  out-of-range codepoint, or a malformed hex escape, is a compile error (#781).
- **`except*` exception-group handling** (#605, D-0129; epic #614 D13). A `try`
  may now use `except* <type> [as NAME]` clauses to split an aggregate
  `ExceptionGroup` — the one a `scope:` raises on two or more concurrent child
  failures (D-0045) — by type. Each `except*` clause peels the sub-exceptions
  matching its type into a sub-group, binds `NAME` to that **group** (always a
  group, even for a single match), and runs its handler; **every** matching
  clause runs (unlike plain `except`, first-match-only), and the unmatched
  remainder auto-re-raises after the last clause — the single survivor **bare**
  (so a plain enclosing `except E` still catches it, honoring Zap's
  bare-for-single rule, D-0045), a fresh `ExceptionGroup` when two or more remain.
  A **bare** exception is matched as a one-element group, so `try/except E` and
  `except*` coexist. A `try` is all-`except` or all-`except*` (mixing is
  `ZAP3001`); `except*` requires a type (a bare catch-all `except*:` is
  `ZAP3001`). The `ExceptionGroup` type is retained — `except ExceptionGroup as
  eg` still catches a whole group — and now **carries its real sub-exception
  objects**, so `eg.exceptions` yields the sub-exception messages as a `list[str]`
  for inspection/iteration (sub-exceptions are kept in a stable sorted order, so
  rendering and `.exceptions` agree regardless of scheduling). `zap fmt`
  normalizes the clause to `except* E`.
- **Display protocol renders interface values** (#707, D-0124; follow-up to
  #274/#706). An interface value — a `{data, itable}` fat pointer whose concrete
  type is erased — now renders through the same one protocol as everything else,
  consistently across `print()`, `str()`, f-strings, the REPL, and doctests,
  instead of the old clean `ZAP6001` deferral: `print(s)` where
  `Shape s = Circle(r=2.0)` renders `Circle(r=2.0)`. **Mechanism:** each
  per-interface dispatch table (itable) gains one **renderer slot** past its
  method slots, holding the concrete type's renderer (the same one a direct render
  resolves — an explicit `__str__`/`__repr__`/`@derive(Repr)`, the tier-3-enum
  structural renderer, or the default field-wise form); a new `IfaceRender`
  instruction loads that slot and dispatches to it with `data` as `self`, exactly
  as interface *method* calls dispatch (D-0016) — no bespoke runtime path, no
  object-header change. The dispatch is dynamic (two concrete types behind one
  interface variable each render as themselves) and recurses into every field type
  — so a tier-1 enum, a **tier-3 data-carrying enum (through #706's renderer)**, a
  tuple, a quoted `str`, and container fields of a struct behind an interface all
  render exactly as on a concrete value; an explicit `__str__`/`__repr__` on the
  concrete type still wins. Only structs sit behind an interface today (interface
  conformance is structural over structs), so enums are rendered through the
  interface as struct fields. The recursive **cycle guard** (the other half of
  #707) remains deferred: a cyclic object graph is still not constructible through
  the supported surface, so there is nothing yet to guard.
- **Display protocol renders tier-3 data-carrying enums structurally** (#706,
  D-0124 extended to D-0046; follow-up to #274). A tier-3 enum value with no
  hand-written `__str__`/`__repr__` now renders through the same one protocol as
  everything else — consistently across `print()`, `str()`, f-strings, the REPL,
  and doctests — instead of the old clean `ZAP6001` deferral error. A
  data-carrying variant renders `EnumName.Variant(field=value, …)` with every
  payload field labelled and rendered in `repr` mode (so a nested `str`/`char`/
  `bytes` payload quotes: `Shape.Circle(r=2.0)`, `Node.Leaf(name='hi', tag='z')`),
  and a payload-free variant renders `EnumName.Variant` with no parentheses
  (`Shape.Unit`). A **recursive** enum (`enum Expr: Add(Expr l, Expr r)`) renders
  recursively through its pointer child. **Mechanism:** one tag-dispatch renderer
  synthesized per enum (memoized) — the tier-3 analogue of the struct default
  renderer, switching on the enum's `i32` tag like the tier-1 `.name` chain — so
  no new runtime support, no GC/scheduler/object-header change. Niche
  `Optional`-shaped enums render via a null test (the `Optional` payload-vs-`None`
  path is unchanged); an explicit `__str__`/`__repr__` still overrides.
  Interface-value rendering was the remaining Display gap and shipped in #707
  (above); only the recursive cycle guard stays deferred.
- **Validated asymmetric-key metadata/import + P-256 ECDSA (ES256) and Ed25519
  sign/verify in `crypto`** (#643, D-0126; builds on #105/D-0056, #628/D-0102).
  A supported, OpenSSL-backed asymmetric-key surface of free functions —
  `crypto.key_info(pem)` (returns a `KeyInfo` value struct: `kind`, `bits`,
  `curve`, `is_private`, with RSA modulus bits so RS256 consumers can enforce
  ≥ 2048), `crypto.rsa_public_key_pem`/`p256_public_key_pem`/
  `ed25519_public_key_pem` (validated public-key import from JWK components),
  `crypto.p256_sha256_sign`/`p256_sha256_verify` (ES256; **JOSE fixed-width
  64-byte `r||s`** signatures — DER is never exposed), and
  `crypto.ed25519_sign`/`ed25519_verify` (**pure Ed25519** over the original
  message, RFC 8032). Algorithm/key-family/role binding is strict: a public key
  cannot sign, a wrong curve/family/length is rejected rather than silently
  reinterpreted, encrypted keys fail deterministically without prompting, signing
  raises catchable `ValueError`, and verification is always a plain boolean. The
  JWK JSON/base64url policy stays in the non-core JWT library; only native key
  construction and cryptographic validation live in stdlib. Unblocks
  zaplang-io/jwt.
- **Constant-time byte comparison in `crypto`** (#729, D-0131; builds on
  #105/D-0056). `crypto.constant_time_eq(a, b) -> bool` reports whether two byte
  sequences (`str` or `bytes`; empty inputs valid) are equal, comparing them in
  time **independent of their byte contents** via OpenSSL `CRYPTO_memcmp`.
  Guarantee: for equal-length inputs the timing does not reveal how many leading
  bytes matched; a length difference returns `false` and the length is
  observable, but no branch is taken on byte contents (the digest length is a
  public constant in the MAC-comparison use case). This gives HMAC/MAC
  verification a vetted constant-time compare instead of a hand-rolled
  XOR-accumulate loop whose timing the compiler/CPU could undermine, unblocking a
  documented constant-time HMAC comparison in the non-core JWT library
  (zaplang-io/jwt#11).
- **`crypto` essentials: KDFs, URL-safe tokens, and the rest of SHA-2** (#762,
  #763, #764, #765; epic #761, D-0132; builds on #105/D-0056). Four primitives
  that close the most-cited gaps from the crypto gap review (Discussion #758):
  - `crypto.pbkdf2_hmac(hash_name, password, salt, iterations, dklen=0) -> bytes`
    — PBKDF2-HMAC password-based key derivation, matching Python
    `hashlib.pbkdf2_hmac` (the first sound password-storage primitive in the
    stdlib; OpenSSL `PKCS5_PBKDF2_HMAC`). `dklen == 0` is the hash's native
    digest length.
  - `crypto.hkdf(hash_name, ikm, length, salt="", info="") -> bytes` — RFC 5869
    extract-and-expand (JOSE ECDH-ES, signed/encrypted cookies, key splitting;
    OpenSSL HKDF). An empty `salt` is the RFC-mandated HashLen zero string;
    `length` is capped at `255*HashLen`.
  - `crypto.token_urlsafe(nbytes) -> str` — URL-safe random token (sessions,
    CSRF, reset links, API keys), CSPRNG bytes base64url-encoded **without
    padding**, mirroring Python `secrets.token_urlsafe`.
  - `crypto.sha384`/`crypto.sha224` (one-shot **and** streaming `_new`/`update`/
    `hexdigest`/`digest`) complete the SHA-2 family, extending the self-contained
    FIPS 180-4 hash core in `zaprt.c` (SHA-384 is truncated SHA-512, SHA-224
    truncated SHA-256; no new dependency). Unblocks the JOSE ES384/RS384/PS384
    family in the non-core JWT library.
  The KDFs accept the SHA family by name (`sha1`/`sha224`/`sha256`/`sha384`/
  `sha512`); `password`/`salt`/`ikm`/`info` accept `str` or `bytes`; an unknown
  hash name, `iterations <= 0`, a negative `dklen`, an out-of-range `length`, or
  a negative `nbytes` raise catchable `ValueError`. Goldens pin RFC 6070
  (PBKDF2-HMAC-SHA1), RFC 5869 Appendix A (HKDF), FIPS/NIST SHA-384/224 KATs, and
  the token shape/alphabet contract. **Experimental** like the rest of `crypto`.
- **`crypto` families: streaming HMAC, `randbelow`, SHA-3/BLAKE2, RSA-PSS,
  extra ECDSA curves, and hex/base64url helpers** (#766–#771; epic #761,
  D-0133; follows D-0132). The nice-to-have follow-ups that round the
  demand-shaped `crypto` surface into deliberate families:
  - **Streaming HMAC** (#766): `crypto.hmac_new(hash_name, key)` /
    `hmac_update` / `hmac_hexdigest` / `hmac_digest` — the incremental
    counterpart to the one-shot HMACs, over the self-contained HMAC core
    (pointer-free handle, non-destructive finalize).
  - **`crypto.randbelow(n) -> int`** and **`crypto.choice(seq) -> T`** (#767):
    unbiased uniform `[0, n)` from the CSPRNG (rejection-sampled), and an
    element-generic uniform pick from a `list[T]`, mirroring `secrets.randbelow`
    / `secrets.choice`. `choice` reuses `random.choice`'s element-type and typed
    element-getter path; an empty list raises a catchable `ValueError`.
  - **SHA-3 and BLAKE2** (#770): `crypto.sha3_256`/`sha3_512`/`blake2b`/
    `blake2s`, one-shot. These come from OpenSSL `EVP_Digest` (so they depend on
    OpenSSL, unlike the self-contained SHA-1/2 + MD5 core); one-shot only for now.
  - **RSA-PSS + larger PKCS#1 v1.5** (#768): `crypto.rsa_pss_sha256/384/512_sign`
    /`_verify` (JOSE PS256/384/512; salt = digest length) and
    `crypto.rsa_sha384/512_sign`/`_verify` (RS384/RS512).
  - **ECDSA P-384 / P-521** (#769): `crypto.p384_sha384_sign`/`_verify` (ES384,
    96-byte JOSE `r||s`), `crypto.p521_sha512_sign`/`_verify` (ES512, 132-byte),
    and `crypto.p384_public_key_pem`/`p521_public_key_pem`.
  - **Encoding helpers** (#771): `crypto.hex`/`unhex` and `crypto.b64url_encode`
    /`b64url_decode` (unpadded URL-safe) — thin `crypto`-namespace aliases over
    the existing `hex`/`base64` runtime, no new C.
  `str`/`bytes` are interchangeable throughout; unknown hash names and bad
  arguments raise catchable `ValueError`. Goldens pin NIST SHA-3 / BLAKE2 KATs,
  RFC 4231 HMAC via the streaming path, RS384/512 fingerprints and
  OpenSSL-produced PSS/ECDSA signatures verified for interop, JOSE widths, and
  the encoding round-trips. **Experimental** like the rest of `crypto`.
- **General value-rendering (repr/Display) protocol for user types** (#274,
  D-0124; follow-up to #252/#159, extends D-0047/D-0046/D-0018/D-0049). A user
  type with no `__str__`/`__repr__`/`@derive(Repr)` now gets an **automatic
  default field-wise rendering** `TypeName(field=value, …)` instead of a compile
  error, and the **same renderer backs `print()`, `str()`, f-strings, the REPL,
  and doctests** — closing the old asymmetry where only `print` could render a
  struct while `str(x)`/`f"{x}"` rejected it. The renderer **recurses through
  every field/element**: integers, `bool` (`True`/`False` — fixing the old
  `-1`-bitpattern bug in `@derive(Repr)`'s bool path), `float`,
  `str`/`char`/`bytes`, `complex`/`bigint`, `Optional[T]`, **nested structs**,
  **tier-1 enums** (as the qualified variant name `EnumName.VARIANT`), **tuples**
  (Python `(v0, v1)` form, one-element tuples keeping the trailing comma
  `(v0,)`), and **`list`/`dict`/`set`** in Python shapes — `[v0, v1]`,
  `{k: v}`, `{v0, v1}`, with the Python-exact empty forms `[]`, `{}`, and
  `set()`. The protocol follows Python's **`str`/`repr` distinction**: a bare
  string at the top level of `print`/`str` is **unquoted** (`hello`), while a
  string nested as a struct field or container/tuple element is **quoted**
  (`'a'`), with `bytes` as `b'…'`. A nested user-type field renders by **calling**
  that type's renderer, so a recursive type graph terminates at compile time (one
  synthesized renderer per layout); containers reuse the existing
  list-iteration + typed dict/set entry points. `@derive(Repr)` now recurses
  through all field types (dropping its old int/bool/str-only limit), and an
  explicit `__str__` (preferred) or `__repr__` still overrides the default.
  Lowering over existing string helpers plus two small added runtime helpers
  (`zap_str_repr`/`zap_bytes_repr`) — **no** GC/evacuator/scheduler/object-header
  change.
  - **Behaviour change:** `@derive(Repr)` previously rendered a `str` field
    **unquoted** (`Point(tag=a)`) and supported only int/bool/str fields; it now
    renders a `str` field **quoted** (`Point(tag='a')`, Python repr) and recurses
    through every field type. Any code depending on the old unquoted form sees
    the quoted form.
  - **Known v1 limitations:** (**`bool` collection elements render
    `True`/`False`**, and **`char` renders Python-style `'a'` in every
    position** — scalar, struct field, tuple element, and collection element /
    dict key / dict value — since #709 made `char` a distinct IR type.) The
    default renderer has no cycle guard (like a hand-written recursive
    `__repr__`); non-printable non-ASCII in a quoted string is not escaped.
    (Tier-3 data-carrying enums, originally deferred here, now render — see the
    #706 entry above; interface values now render too — see the #707 entry above.)
    Still deferred to a #274 follow-up: the recursive cycle guard (#707), latent
    until a cycle is constructible. No object-identity (`<T @ 0x…>`) form is
    introduced.
- **`char` is a distinct IR type — fixes `char`/`int32` render conflation**
  (#709, D-0125; completes D-0124). `char` previously shared the IR type
  `Ty::I32` with `int32`, so the value layer could not tell them apart: a `char`
  in a collection rendered as its decimal codepoint (`list[char]` → `[97, 98]`),
  and — symmetrically — an `int32` *scalar* or *struct field* rendered as a
  *character* through `str()`/f-strings. `char` is now its own IR type
  (`Ty::Char`), physically the same `i32` codepoint with byte-identical
  arithmetic / comparison / coercion / hashing / codegen / GC, distinguished
  only so the Display protocol renders it correctly: a `char` renders
  Python-style (`'a'`) in **every** position — scalar, struct field, tuple
  element, list/set element, and dict key **and** value — while an `int32`
  renders as a **decimal** everywhere. `char` arises from `s[i]`, `chr()` /
  `char()`, and string iteration (there is no `char` literal). `print(c)` of a
  `char` now shows the character (e.g. `print(s[0])` → `H`) rather than its
  codepoint. **Invariants held:** `char` as a dict key / set element hashes and
  compares byte-identically (the D-0122 key path, `_i64` entry points); `char`
  arithmetic/comparison/coercion and all `int32` behaviour are unchanged except
  the rendering fix.
- **Tuples are hashable — usable as dict keys and set elements** (#680, D-0122;
  refines D-0049/D-0047). A tuple of hashable elements is now a valid dict key
  and set element with **structural** hash/equality (CPython parity):
  `{(1, 2): 7}`, `dict[(int64, str), int64]` key operations, and the set literal
  `{(1, 2)}` all work, including **nested tuples** `((1, 2), 3)` and tuples of
  `str`/`bytes`/`bool`/`char`/`enum`/hashable-struct elements. A tuple already
  lowers as an anonymous value-struct aggregate and value-struct keys already use
  the function-pointer key path, so the compiler **synthesizes** the tuple's
  structural `__hash__`/`__eq__` over its element layout (the same machinery
  `@derive(Hash, Eq)` uses), generated once per layout and reused — **no** GC,
  Immix, object-header, or C-runtime change. A tuple with an **unhashable**
  element (a `float`, a nested `list`/`dict`/`set`, or a struct lacking
  `__hash__`/`__eq__`) is rejected at compile time with a stable coded diagnostic
  (**ZAP6001**), never an internal compiler error. Previously every one of these
  forms **panicked the compiler** (ICE in `key_fn_args`, a tuple's interned
  struct id having no name entry). (The typed empty set constructor
  `set[(int64, int64)]()` and `Optional[T]` keys — flagged here as follow-ups —
  are now closed by D-0123, below.)
- **Composite-element collection constructors, and `Optional[T]` as a dict key /
  set element** (#703, #704, D-0123; closes the two D-0122 follow-ups). Two
  completeness items, fixed together — both sema/lowering, reusing the key-fn
  path, with **no** GC/Immix/object-header/C-runtime change and no grammar
  change. (A) The typed empty **collection constructors** `set[T]()` /
  `channel[T]()` / `heap[T]()` now accept any valid element type expression, not
  just a bare name: `set[(int64, int64)]()` (and nested-tuple / `(int64, str)`
  elements) work, consistent with the literal `{(1, 2)}` and `dict[(..), ..]`
  keys; `set[list[int64]]()` is still the clean unhashable reject, and
  `heap[(..)]()` / `channel[(..)]()` remain clean rejects (a heap is ordered, a
  channel stores uniform words) rather than the old misleading "expects an
  element type name". (B) **`Optional[T]`** (for hashable `T`) is now hashable
  and usable as a dict key / set element with **structural** hash/equality
  (CPython parity): `dict[Optional[int64], str]` with both a value key and a
  `None` key works — including the bare `None` literal in key position
  (`d[None]`, `None in d`, `s.add(None)`) — `None` hashing to a sentinel and
  comparing equal only to another `None`, a present value delegating to `T`.
  Tagged and niche Optional layouts and nested payloads (`Optional[(int, int)]`)
  are handled; an `Optional` of an unhashable `T` (e.g. `Optional[float]`) is a
  clean `ZAP6001` reject, never an ICE. Previously an Optional key rejected with
  `ZAP6001 "Phase 1 slice"`.
- **`spawn` of methods and first-class function values** (#475, D-0118; epic
  #469). `spawn` now accepts three callable forms, closing the gap where the
  grammar and SPEC admitted method/closure spawns but lowering rejected them:
  (1) a **named function** (`spawn worker(ch)`), (2) a **user-struct method
  call** (`spawn server.handle(conn)` — the receiver is captured as the first
  argument), and (3) a **first-class function value** (`spawn f(x)` where `f` is
  a variable, field, or expression of `fn(...)->...` type: a bound lambda, a
  closure, or a function value, lowered through a generated per-signature thunk
  that performs the indirect call). The receiver, arguments, and closure are
  captured **eagerly in the parent** (D-0034); a value-struct receiver/argument
  is copied so it outlives the frame, and a reference/closure capture is a GC
  pointer traced through the spawn environment. All forms are **positional-only**
  (no keyword, spread, or default filling), and every new form composes with the
  existing `scope:` join, cancellation, `finally`/`with` cleanup, failure
  propagation, and `scope(timeout=…)` (D-0094) — a `ScopeTimeout` pending report
  now names a method as `Struct.method` and a function value by its callee. A
  bare lambda *literal* operand (`spawn (lambda: ...)()`) stays a compile error
  (**ZAP5076**): bind it to a name first. Keyword/spread arguments, an arity
  mismatch, and a non-callable or non-struct-method operand are also ZAP5076.
- **Bare `raise` (re-raise) in an `except` handler** (#657, D-0116; Python
  parity D-0007). A bare `raise` — no operand — inside a handler now re-raises
  the exception currently being handled, **preserving its type and
  value/identity** (it re-throws the very object the handler caught, through the
  same `zap_raise` unwinding entry a bound `raise <obj>` uses, so a re-raised
  `ValueError` is still caught by an enclosing `except ValueError`). The
  innermost active exception is re-raised, so a bare `raise` in a nested handler
  re-raises that nested handler's exception. Previously this was rejected at
  lowering with the generic `ZAP6001` umbrella. Where there is **no active
  exception** — a bare `raise` in a `try` body, an `else`/`finally` not handling
  an exception, at top level, or in a function merely *called* from a handler —
  Zap rejects it at **compile time** with a dedicated diagnostic **ZAP6002**
  rather than deferring to Python's runtime `RuntimeError`; Zap tracks the
  active exception lexically and per function, so these cases are statically
  provable and it does not implement Python's dynamic, cross-call-frame re-raise
  in v1. Full traceback fidelity across the re-raise rides on the green-stack
  unwinder (I#475) and is a follow-up; type and value are preserved today. The
  grammar was unchanged (it already admitted a bare `raise`); this was a
  lowering gap.
- **`@cache` built-in decorator — memoization over module-level state** (#666,
  D-0115; closes the language work of epic #661, supersedes the D-0109 `@cache`
  deferral). `@cache` on a free function memoizes it by the typed argument tuple,
  generating **ordinary, `zap expand`-visible, type-checked Zap over a
  module-level dict** — never a hidden compiler store. Expansion (before sema, in
  the same pass as user `before`/`after`/`wrap`, so it stacks bottom-up)
  synthesizes a `@derive(Hash, Eq)` key-struct (the argument tuple, as a named
  struct — a bare tuple dict key ICEs, #680), a `@shareable` store struct holding
  a `Mutex` and the memo `dict` (so the shared mutable store does not trip the
  #664 ZAP5125 lint), an immortal module global, and a wrapper that returns a hit
  or computes-and-stores a miss. The miss is computed **outside** the lock, so a
  recursive `@cache` function cannot self-deadlock; the lock guards only the dict,
  like `functools.lru_cache`. v1 is unbounded for the process lifetime (a
  documented memory cost). The key must be **value-hashable** — an integer,
  `bool`, or `str` in v1; a **reference type** (a `struct`/`enum` or a
  collection) is a **compile error (ZAP5056)**, not silently keyed by identity
  (which would miss on equal-but-distinct arguments and give no benefit). An
  un-memoizable signature — a non-value-hashable parameter (reference,
  `float`/`bytes`, tuple, …), a variadic/`**kwargs`, zero-parameter, generic, or
  no-return function, a method, or any `@cache` argument — is a stable
  **ZAP5056** error (never an ICE). The
  sema `@cache` arm no longer rejects with ZAP5055 (that was the deferral); it now
  only catches misapplication to a non-function target.
- **Runtime data-race detector surface: `--race`** — the *dynamic* backstop for
  the light concurrency model (#665, D-0114; locked-design point 6 of epic #661,
  complementing the static sharing lint from #664). A new **`--race`** flag
  (alias **`-race`**) on `run`/`build`/`ir`/`test` enables the runtime data-race
  detector: a **test-time** diagnostic, off by default, that never changes what
  the language accepts. D-0114 settles the design — a **Zap-native happens-before
  / vector-clock** detector rather than LLVM/Clang ThreadSanitizer (which assumes
  pthreads and `malloc`, not Zap's user-space coroutine scheduler and GC heap) —
  and scopes it to the ZAP5125 set (a mutable, non-`@shareable` module global), so
  an `Atomic`/`Mutex`/`@shareable`-guarded version is clean. The feature is being
  delivered in slices: earlier work landed the design decision, the flag plumbed
  end-to-end, and the racy-vs-clean fixture pair (`tests/race/`) as the detector's
  walled-garden spec. **Slice 1 (#677) now lands the access instrumentation:**
  behind `--race`, AST→ZIR lowering emits a `__zap_race_read` / `__zap_race_write`
  hook at exactly the ZAP5125 set (a mutable, non-`@shareable` module global —
  scalar *or* reference; not a `const`, a `@shareable` cell, a shadowing local, or
  the module-init pre-pass), calling runtime **no-op stubs** (`zaprace.inc.c`) for
  now. With `--race` **off**, lowering emits nothing and the output is
  byte-identical to before, so default builds and every golden are unaffected; a
  `--race` build differs only by the inert calls. **Slice 2 (#678) now makes the
  detector live:** a **Zap-native happens-before / vector-clock engine**
  (`zaprace.inc.c`) replaces the stubs. Each coroutine carries a vector clock,
  each `Mutex` a release/acquire clock, and each instrumented global shadow state
  (per-coroutine write/read clocks); happens-before edges are taken from **spawn**
  (parent → child) and **`Mutex` lock/unlock**, with `Atomic` clean by exemption.
  A conflicting access (write–write or read–write) with no ordering edge is
  reported — naming the global and both accesses' source spans — and the process
  exits non-zero (66); the verdict is schedule-independent, so the report is a
  stable golden. All engine state is serialized under one detector lock (obvious
  correctness over speed; safe under `MAXPROCS>1`). `tests/race/` gains a
  read/write and a write/write racy variant, and all six fixtures are CI-gated
  goldens under both collectors (`racy_*` report a race on `counter`; `clean_*`
  run clean and print their exact total). **Slice 3 (#679) now widens the
  detector:** happens-before edges are added for **channels** (a send → the
  matching receive, for both an unbuffered/rendezvous and a buffered hand-off),
  **`WaitGroup`** (every `done()` → a `wait()` that returns), and **`Once`** (the
  winner's `once_end` → every other caller's `once_begin` return) — all wired in
  the runtime, so a non-`--race` build stays byte-identical. The single-lock
  engine is validated under **`ZAP_MAXPROCS>1`** (true parallelism) across the
  whole fixture set (a second CI golden), and the race report now also names each
  racing access's **coroutine function**. `tests/race/` gains `clean_chan_*`,
  `clean_waitgroup`, `clean_once`, and `racy_chan` (a soundness guard that a
  channel does not fabricate an edge between two coroutines racing on a global),
  plus a host-safe guard that `zapc ir --race` actually emits a hook (tripwire
  for a dropped-flag regression). **Slice 4 (#684) now extends the detector to
  shared heap objects:** behind `--race`, lowering emits the access hooks at
  user-source **`struct` field** and **`list` element** load/store sites too, so a
  race on a shared heap object — two coroutines mutating the same list element or
  struct field with no synchronization — is reported with a source location and
  exits non-zero, while a `Mutex`- or `concurrent`-collection-guarded version is
  clean. Heap shadow state is keyed on the object's **stable header identity** (a
  field/element index off the header pointer), not the relocatable element
  address, so it survives precise-Immix evacuation (#120); a `zap_race_on_move`
  evacuator hook is the general relocation path and keeps the design
  collector-agnostic (Boehm never moves). The happens-before engine and single
  lock are unchanged — this slice only widens *what* is instrumented. New
  `tests/race/` fixtures (`racy_heap_list`, `racy_heap_struct`, `clean_heap_mutex`,
  `clean_heap_concurrent`, and `clean_heap_evac` — a shadowed object kept live
  across an evacuating GC) run under both collectors and `ZAP_MAXPROCS>1`. The
  **main/native thread's own accesses** stay untracked (a main-vs-coroutine race
  is missed; races between spawned coroutines are caught); **dict/set element** and
  **interface/boxed-payload** heap coverage and full multi-frame stack traces (the
  I#475 green-stack unwinder) remain follow-ups.
- **Concurrency-safety surface: `@shareable` + sharing lint** — the advisory
  surface for the light concurrency model (#664, D-0113; locked-design points
  4–5 of epic #661, building on the mutable globals from #662). `@shareable` is a
  no-argument marker on a `struct` asserting — as a *trusted, unverified* claim —
  that the type is safe under concurrent shared access; it is reflectable via
  `reflect_has(T, "shareable")`, and the built-in `Atomic`/`Mutex`/`WaitGroup`/
  `Once`/`Counter`/`Queue` types carry it implicitly under one uniform rule (no
  hardcoded allowlist in the lint). The static **sharing lint** raises the
  non-fatal warning **`ZAP5125`** at each read/write of a **mutable** module
  global whose type is not `@shareable`; a `const` global never warns, and a
  mutable global of a `@shareable` (or built-in shareable) type is silent. A
  per-site **`# noqa`** (or `# noqa: ZAP5125`) comment suppresses one site. It is
  advisory — the program stays legal — never a dialect. A new
  **`--warnings-as-errors`** flag (alias **`-Werror`**) on `run`/`build`/`ir`
  makes any surviving warning fatal for CI; off by default.
- **Precise-Immix static roots + write barrier for module globals** (#663,
  D-0112; epic #661). Module-global pointer slots are now correct under the
  **precise Immix** collector, not only conservative Boehm. Every pointer slot is
  registered in the precise collector's static root table, which it walks at each
  GC and processes **move-aware** — marking the referent and rewriting the slot
  in place when the referent is evacuated — so a structure held only via a module
  global is never collected and never left with a stale pointer after relocation.
  Reassigning a **mutable** module-global pointer (`=`, `+=`, chained, tuple-leaf)
  now emits a `zap_gc_write_barrier` on the slot; an immutable reference `const`
  and the one-time initializer store emit none. The barrier is a linkable runtime
  no-op today (the stop-the-world collector re-scans every root each cycle) and is
  the reserved record point for a future generational/incremental mode. Boehm
  behavior is unchanged (the barrier and root registration are exported no-ops;
  the data segment is scanned conservatively).
- **Module-level mutable globals** — persistent, reassignable state at module
  scope (#662, D-0112; Phase 2 of the module-level-persistent-state epic #661,
  building on the `const` form from #658). A `T NAME = expr` (or
  `auto NAME = expr`) at module top level — scalars and reference/heap types
  alike — is evaluated **once before `main`** in the same import-topological +
  source order as `const`, is a module member reachable across modules as
  `module.NAME`, and may then be **reassigned** (`=`, `+=`) and **mutated**
  (container contents) from any function, persisting for the process lifetime.
  There is no `global` keyword: a bare assignment to a name that resolves to a
  module binding reassigns it in place (static lexical scoping, D-0021/D-0090).
  A reference-typed mutable global is a **permanent GC root** (its data-segment
  slot address is registered, so the collector keeps whatever heap object it
  currently holds live — including after reassignment or growth — under both the
  conservative Boehm and precise Immix collectors). Concurrency is the light
  Go/Python model of #661 (unsynchronized, never implicitly locked); the static
  sharing lint and the `@shareable` marker now ship (#664, above), and the
  precise-Immix static roots and write barrier landed in #663 (above), while a
  runtime `-race` detector is later work. A module-level tuple-destructuring
  declaration stays rejected
  (**ZAP3018**); a `const` module binding stays immutable (**ZAP3017**).
- **Module-level `const`** — persistent, immutable state at module scope (#658,
  D-0112; Phase 1 of the module-level-persistent-state epic #661). A
  `const T NAME = expr` (or `const auto NAME = expr`) at module top level, for
  scalars and reference/heap types (`const list[int64] XS = [...]`,
  `const dict[str, int64] TABLE = {...}`), is evaluated **once before `main`** in
  import-topological + source order (reusing the acyclic module graph, D-0081),
  is a module member reachable across modules as `module.NAME`, and is a
  permanent GC root when reference-typed. It is immutable: reassignment is a
  compile error (**ZAP3017**) — which also closes a latent gap by enforcing
  `const` immutability for **local** `const`s. A module-level *mutable* binding
  is reserved for #658 Phase 2 and is rejected with a dedicated **ZAP3018**
  (pointing at the planned support) rather than the blanket top-level-statement
  error. The broader design — mutable module state, a static concurrency-sharing
  lint, and the `@shareable` marker — is epic #661.
- **`as` sub-pattern capture in `match`** — `case <pattern> as name:` (#603,
  D-0111; CPython grammar-parity epic #614, D11). It matches the inner pattern
  and binds `name` to the *whole* matched value, typed as the scrutinee for that
  arm. The inner pattern still destructures, so nested captures bind alongside
  the `as` name (`case [x, y] as pair:` binds `pair`, `x`, `y`;
  `case Point(x=0) as origin:` binds `origin`), and the `as` wraps the entire
  or-pattern (`case 1 | 2 | 3 as n:`). The capture is in scope in the arm body
  and its guard (`case [x, y] as pair if len(pair) == 2:`). `name` must be a
  single plain identifier — not `_`, not dotted — rejected otherwise with the
  dedicated **`ZAP2021`**. Matches CPython's `as`-pattern surface.
- Built-in **`@deprecated`** decorator (#472, D-0109, D-0036). A function or
  method marked `@deprecated` (optionally `@deprecated("use X")`) generates no
  code but raises a non-fatal **use-site warning** (`ZAP5124`) at every call
  site — the mechanism for retiring a Stable surface with notice before removal
  (STABILITY §5). Sema warnings are now surfaced by the build pipeline at a new
  `warning[ZAPnnnn]` severity (non-fatal), which also surfaces the pre-existing
  `Eq`-without-`Hash` dict/set-key warning (D-0047) at the CLI for the first
  time. With this, the v1 built-in decorator set is complete: `@derive(...)`,
  `@default`, `@validate`, `@range`, `@deprecated`.
- Identity operators `is` / `is not` (#611, D-0110, epic #614 D18). `a is b`
  tests **reference identity** — same allocation — distinct from `==` (value /
  `__eq__` equality): two distinct reference structs with equal fields are
  `==`-equal but not `is`-equal. `x is None` / `x is not None` is the
  empty-variant / null check, consistent with `== None` and `match case None`,
  and works for both `Optional[T]` layouts and plain reference structs. `is`
  reserves a new keyword (as in Python) and, like the other comparison
  operators (and unlike `not in`), participates in Python-style comparison
  chaining (`a is b == c`). Operands must be reference types (a plain `struct`,
  a `list`/`dict`/`set`/`heap`, a `channel`, or `Optional[T]`); `is` on a value
  type or scalar is a compile error (ZAP4073), as is `is None` on a never-None
  value (ZAP4074), each pointing the user at `==` — Zap does not define `is` as
  `==` on values (see `PYTHON_DIFFERENCES.md`).
- Generator expressions: `(elem for CLAUSES)` is a lazy `Iterator`, not a
  materialized collection — it yields each element on demand (no intermediate
  list) and can be consumed once. It uses the comprehension clause grammar
  (multi-`for`/`if`), its loop variables do not leak into the enclosing scope,
  and as the sole argument of a call it needs no parentheses of its own
  (`sum(x*x for x in range(5))`; `f((x for x in xs), y)` with other arguments).
  It desugars to an anonymous generator function reusing the `yield` state
  machine, so it integrates with `for`/`sum`/etc. through the one iteration
  protocol. `sum(...)` now also accepts any iterable (generator, genexpr, list),
  not only numeric arrays (#596, D-0107, epic #614 D4).
- User-defined **behavioral decorators**, `before`/`after` form (#652 of epic
  #651, D-0108, D-0036). A `decorator` may declare `before(ctx)` and/or
  `after(ctx, result)` hooks; applying it to a free function generates — at
  compile time, before type-checking, visible via `zap expand` — a wrapper with
  the decorated function's **exact signature preserved** (automatic `@wraps`)
  that runs `before`, calls the original, runs `after` on the return value, and
  returns it. `ctx.name` is the decorated function's name; decorators stack
  bottom-up; decorator arguments (`@tag("x")`) wire into the generated code.
  Ill-formed uses are stable diagnostics (ZAP5049–ZAP5053);
  behavioral decorators on non-function targets report ZAP5050.
- User-defined **behavioral decorators**, general `wrap(call)` form (#653 of epic
  #651, D-0108, D-0036). A `decorator` may declare a `wrap(call)` hook whose body
  is a nested `def inner(*args): … return inner`; applying it generates — at
  compile time, visible via `zap expand` — a wrapper with the decorated
  function's **exact signature** in which the inner body is inlined and
  `call(*args)` becomes a typed call of the renamed original forwarding the
  decorated function's parameters. `*args` is a compile-time forwarding template,
  **not** a runtime varargs value (so it may appear only as `call(*args)`;
  reading it, using `call` as a value, mixing `wrap` with `before`/`after`, or
  omitting `return inner` are ZAP5052/ZAP5054). A declared-return wrapper whose
  body is not provably return-terminating (e.g. `retry`) gets an implicit
  `raise RuntimeError(...)` fall-through guard. `wrap` stacks with other
  decorators bottom-up. (An `args`-keyed `memoize`/`@cache` is not expressible by
  `wrap`: it needs persistent cross-call state, a separate capability.)
- Transparent type aliases: `type Name = <type>`, plus the generic form
  `type Name[T] = <type>` (#606, D-0105). An alias is not a new nominal type — it
  resolves to its target with the same identity, so an alias and the spelled-out
  type are interchangeable in signatures and collections (`list[Row]`,
  `dict[str, Row]`). Generic aliases are expanded during monomorphization
  alongside the existing generics (`Pair[int64]` → `(int64, int64)`). `type` is a
  soft keyword recognized only at item start, so it stays a usable identifier
  elsewhere. A cyclic alias (`type A = B; type B = A`) is rejected (`ZAP4090`).
- Positional-only parameters via a bare `/` separator: parameters before `/`
  may only be supplied positionally, never by name (the mirror of the keyword-only
  `*` separator). `/` appears at most once and must precede any `*`; passing a
  positional-only parameter by name is `ZAP4034`, a misplaced `/` is `ZAP2019`.
  Enforced front-end only — no IR/codegen change (#602, D-0104, epic #614 D10).
- Walrus assignment-expression `(name := expr)`: evaluates `expr` once, binds
  `name` by inference in the lexically enclosing scope, and yields the value, so
  the name outlives the construct — usable in `if`/`while` conditions and
  comprehension `if` filters (`if (n := len(xs)) > 10:`, `[y for x in xs if
  (y := f(x)) > 0]`; a comprehension target binds in the enclosing scope per PEP
  572). It is parenthesized-only and deliberately not a declaration shorthand: a
  bare `a := 10` statement is rejected (`ZAP2020`, steering to `auto a = 10`)
  (#600, D-0106, epic #614 D8).

- Array length mismatches raise `ValueError` before SIMD kernels run; the
  Experimental array contract records supported 1-D dtypes, construction,
  arithmetic and contiguous views. Unsupported array constructors and native
  floating-point collection slots have semantic diagnostics (#476).

- `scope(timeout=<ms>)` deadlines (integer milliseconds, no unit suffix): on
  expiry the scope cancels and awaits its children through the shared channel-aware
  cancellation path (preserving `finally`/`with` cleanup), then raises the new
  `ScopeTimeout` exception to the caller. `ScopeTimeout` reports the deadline and
  every coroutine still outstanding at it (by name and park reason), never a sample.
  `timeout`, `on_error`, and `cancel_on` are orthogonal; `grace` still bounds an
  uncooperative drain. Nested joins park their coroutine (#475, D-0094).

- Scope cleanup cancels and awaits children on return/break/continue; unsupported
  spawn call forms now report `ZAP5076` during semantic checking (#475). The
  concurrency contract documents supported scope options without a timeout API.

- Structural `match` lowering for tuples and lists with rest captures, recursive
  enum/struct payload tests and bindings, and guard fallthrough (#474).

- **Language.** Type-first declarations and `const` (D-0003); scalars and
  conversions, including arbitrary-precision `bigint` and native `complex`
  arithmetic (D-0019); `str`/`bytes` with interpolation, codepoint indexing/slicing/
  iteration, comparison, substring `in`, the core method set
  (`upper`/`lower`/`strip`/`split`/`join`/`find`/`index`/`replace`/
  `startswith`/`endswith`/`contains`/`encode`) and `ord`/`chr`/`char`/`str`/
  `int` conversions (#284);
  `list`/`dict`/tuples/`set` with value/reference semantics; structs, methods
  and `value struct` (D-0051); interfaces; enums — tier-1 values with
  canonical `.name` and source-order iteration, plus tier-3 sum types
  (D-0018/D-0046); operators with overloading and `@derive(Eq, Ord, Hash, Repr)`;
  control flow including `match` and the iteration protocol (D-0048);
  functions, lambdas, generators, default/keyword/variadic args, comprehensions
  and slicing; first-class function values and closures (D-0067); typed built-in
  exceptions with source-order `except` dispatch and subclass matching; `with`/
  context managers (D-0069); modules, imports and visibility; regex
  (D-0010); decorators and annotations (D-0036); generics with bounds (D-0064);
  coroutines, channels, `select`, `scope:` and `spawn detached`
  (D-0031/0044/0045); sync primitives and `Atomic[T]` (D-0032/0066);
  `Optional[T]` (D-0065); numeric arrays / SIMD with dtype promotion (D-0011);
  C-style bitfields (D-0068); Python interop; `@derive(Json/Yaml/Csv)`; and
  `when target.*` compile-time conditional compilation (D-0079, Experimental).
- **Standard library.** `io`, `fs`, `os`, `sys`, `process`, `math`, `random`,
  `time`, `datetime` (incl. `strftime` / 2-arg `format` custom date formatting,
  #286); `collections` extras (`deque`, `bisect`, `oset`), `sync`,
  `json`, `logging`, `argparse`; the `signal`/`gc`/`runtime` APIs; and the
  Experimental `crypto`, `net`, `http`, `tls`, `zlib`, `gzip`, `yaml`,
  `xml` — a core DOM parser + deterministic serializer for the AWS rest-xml /
  query protocols (D-0085, #351);
  migration-only `threading`, and `concurrent` lock-free `Counter` + MPSC
  `Queue[T]` (D-0078).
- **Runtime & GC.** Boehm and precise moving-Immix collectors with finalizers
  and evacuation (D-0009); the coroutine scheduler with preemption; the
  netpoller (epoll / IOCP); signal-triggered preemption and runtime diagnostics.
- **Tooling.** The `zap` CLI (`run`, `build`, `test`, `fmt`, `new`, `init`,
  `add`, `remove`, `zenv`, `explain`, `doc`, `version`, `help`); `zap fmt`
  canonical formatter (D-0037); doctests via `zap test --doc` (D-0074); the
  `ZAPnnnn` diagnostic scheme with `zap explain` (D-0071); the LSP server and
  VS Code extension; the in-process-JIT REPL (D-0073); the web playground
  (D-0075); the MCP server; and the package manager with `zenv`, MVS resolver,
  lockfile, checksum DB and read-only registry index (D-0052/D-0027).
- **Platform.** Linux x86-64 and Windows x86-64 (tier-1); macOS x86-64
  (tier-2, CI runner pending); aarch64-Linux and macOS arm64 (experimental).
  See the ratified matrix in STABILITY §7.3.
- **Process & docs.** `SPEC.md`, `docs/GRAMMAR.ebnf`, the `DECISIONS.md` log,
  `docs/STABILITY.md` versioning & stability policy (D-0070/D-0071) with the
  D-0076 1.0 spec-freeze, and this changelog plus the release process
  (`docs/RELEASING.md`, I#207).

### Changed

- **Collection element checks are capability/representation-driven** (#807, epic
  #812, D-0139). The hand-maintained per-container element guards fold into the
  capability model: a container's element/key/value position carries a semantic
  bound (`dict`/`set` key = `Hashable`, `heap` element = `Comparable`,
  `list`/`array`/`channel` + `dict` value = none), and every type admitted today
  satisfies it (`f64` included), so what actually gates admission is
  representation readiness. `f32`/`float32` as a collection element is now
  diagnosed as an honest **representation-readiness gap** — *"'f32'/'float32' is
  not supported as a collection element yet (only 'f64' is); use 'array[f32]' for
  dense f32 data"* (`ZAP4027`, retired from its old "floating-point collection
  elements are unsupported" enumerated-float wording) — rather than a type law;
  `bigint`/`complex` stay rejected unchanged (`ZAP4024`). No behaviour change for
  any currently-accepted shape (all #800/#803 `f64` cases still type-check); the
  enumerated float arms are gone in favour of the uniform bound/representation
  walk.
- **The three aarch64 concurrency stress goldens run on every arch again**
  (#698, D-0121). `immix_evac_coro_stress`, `concurrent_queue_mpsc_coro_stress`
  and `concurrent_queue_mpsc_thread_stress` were quarantined on aarch64 via
  `.skip_arch` sidecars (PR #445) solely because they intermittently **hung** on
  the parked #223 scheduler deadlock. That deadlock is fixed (PR #694, D-0117),
  so the sidecars are deleted and the goldens are no longer skipped on aarch64.
  CI runs each once through the normal harness with **no** `GC_FREE_SPACE_DIVISOR`
  pressure knob, at which point all three are clean; the divisor-only OOM that
  `concurrent_queue_mpsc_coro_stress` could hit is now a clean abort, not a
  SIGSEGV (#697/#699, D-0119), and CI never sets that knob. No other test was
  weakened, quarantined, or skipped.
- Built-in decorators `@cache` and `@inject` are out of the v1 set (#472,
  D-0109). **`@cache` is deferred** — it needs module-level persistent state,
  not yet a language feature (tracked in #658/#661; the built-in in #666).
  **`@inject` is declined for core** — dependency injection is a framework
  concern, provided by reflection-driven `Depends(...)` adapters (D-0089/D-0092),
  not a core decorator. Both names stay reserved and are rejected with a
  dedicated diagnostic (`ZAP5055`) — for `@inject`, a redirect to `Depends(...)`
  — instead of being advertised and then failing at lowering.

### Deprecated

- _Nothing yet._

### Removed

- _Nothing yet._

### Fixed

- **Indexed store of a `bool`/`char`/`int` collection element rejected with a
  self-contradictory "expects `i64`, found `i64`"** (#727; regression from #708,
  widened by #712). `int`, `bool` (`Elem::Bool`) and `char` (`Elem::Char`)
  collection elements are one physical cell — an `i64` word, the `i64` setter
  suffix, an `i64` read-back — distinct `Elem` kinds only so the Display protocol
  can render them as a decimal, `True`/`False`, or `'a'`. A value read back from
  such a collection therefore carries the bare `i64` read-back type, so
  `want[i] = flags[j]` (and the issue's `objects[depth] = True` /
  `want_key[depth - 1] = objects[depth - 1]`) stored an `i64`-word value into a
  `list[bool]`/`list[char]` element. `check_index_value` compared exact `Elem`
  identity, so `Elem::I64` (the stored value) ≠ `Elem::Bool`/`Elem::Char` (the
  destination) was rejected — and since both render `i64`, the message named the
  same type on both sides. Before #708 added the `Elem::Bool` render tag these
  elements were all `Elem::I64`, so the store was accepted; the store path still
  coerces the word to the destination cell and emits the `i64` setter regardless,
  so this was purely a bogus front-end rejection. The check now treats the
  integer-word family (`i64`/`bool`/`char`) as mutually store-compatible while
  keeping exact identity for every other kind (`Elem::Enum`, `Elem::Ptr`,
  `Elem::Struct`, `Elem::F64`, nested collections — a genuine type mismatch such
  as a `str` into a `list[int]` or one enum into another still errors). Covered
  by the `list_index_store_int_word` / `list_index_store_composite` goldens and a
  `tests/diff` CPython case.
- **gc-boehm multicore hang + queue-node use-after-free: the fairness monitor no
  longer preempts a worker during a stop-the-world** (#710, D-0128; follow-up to
  #223). The #172 monitor sent its `SIGURG` fairness-preempt signal to a worker M
  unconditionally, including while bdwgc was mid-stop-the-world. Under gc-boehm at
  high core count (`MAXPROCS≈32-64`, `GC_FREE_SPACE_DIVISOR=3`, on **both** aarch64
  and x86_64) that raced the collector's suspend/ack handshake — a **hang** with
  `main` spinning in `GC_generic_malloc_many` while every worker sat in bdwgc's
  suspend handler — and, via the handler's in-signal `copreempt` stack-swap landing
  as a collection began, moved a mutator out from under the mark, reclaiming a live
  `concurrent.Queue` node (a **use-after-free** surfacing on the consumer as
  `called unwrap on an Optional.None value`). The monitor now gates the signal on
  the seq-cst `g_boehm_in_stw` flag, so preemption still fires for fairness but
  never into a live collection; the handler's existing #700/D-0120 re-check closes
  the sub-microsecond residue. Default gc-immix has no signal-based stop-the-world
  and is unaffected. The three aarch64 `.skip_arch` stress goldens
  (`concurrent_queue_mpsc_coro_stress`, `concurrent_queue_mpsc_thread_stress`,
  `immix_evac_coro_stress`), re-quarantined in #711 after #698 proved premature,
  run on every arch again; a new walled-garden golden
  `concurrent_queue_gc_preempt_stress` exercises the preempt-vs-STW overlap under
  both collectors. Root cause was box-confirmed on a 32-vCPU x86 host (disabling
  the signal ran the coro stress 800+ iters clean vs. failing by ~iter 100);
  final 0-failure confirmation of the gated build at scale is a many-core re-run
  (the maintainer's Graviton acceptance step), the same posture as D-0117/D-0120.
- **Weak-memory audit: cross-thread runtime flags are now `__atomic`, not plain
  `volatile`** (#213, D-0121). A plain `volatile` guarantees nothing across
  threads — x86's TSO hides this, but AArch64's weaker model does not — so the
  runtime's genuine cross-thread signal flags, those read and written by
  different threads with no mutex ordering either side, were converted to
  `__atomic_*` with a documented ordering (building on D-0120, which already did
  this for the one STW guard). Converted, all **RELAXED** because each is a pure
  signal with no data payload: the preemption monitor's `M.schedtick` (monotone
  liveness counter) and `M.preempt` (advisory "please yield" latch, also set from
  the SIGURG handler — `__atomic` on an aligned int stays async-signal-safe and
  lock-free), the one-shot exit latches `g_shutdown`, `g_np_shutdown` and
  `g_sig_shutdown` (their real liveness/ordering already comes from the paired
  broadcast/kick + join, not the flag), and the `g_sig_delivered` async-handler
  test-sync counter. GC-root safety of a preemption is independent of these
  flags' ordering (D-0117), so RELAXED is provably sufficient throughout.
  Deliberately **left as-is** (each documented in place): `Waiter.done` and the
  dedicated-M `ded_runnable`/`ded_stop` flags are fully ordered by their channel
  / `ded_mu` mutex on both sides; `g_np_started` and `g_sig_active` are ordered
  by `pthread_once` init and only mutated at init / post-join shutdown; the
  netpoller self-test's `g_np_t_byte`/`g_np_t_status` are ordered by the channel
  send/recv that frames them; the Windows-only `in_hijack` uses the Win32
  interlocked hijack primitive. No known failure was traced to these — this is a
  weak-memory correctness hardening with no behavior change, verified green by CI
  (the C runtime cannot be built on the authoring host).
- **Out-of-memory is now a clean abort, not a nil-deref crash** (#697, D-0119).
  Generated code stores into an allocation result without a null check, so on
  heap exhaustion — where gc-boehm's `GC_MALLOC` returns `NULL` (bdwgc's default
  OOM handler returns `0`) — the runtime handed `NULL` to that store and faulted
  as a wild SIGSEGV at address 0 (pinned from a Graviton core, #650: a
  `zap_alloc(8)` returned `NULL` under the stress harness's artificial tight-heap
  knob and was dereferenced unchecked). The allocator wrappers now treat a `NULL`
  result as out of memory and print `zap: out of memory` then `abort()` with a
  nonzero exit. Collector-agnostic: one shared `zap_oom` helper is the single exit
  for both the boehm `GC_MALLOC`-NULL path and the immix map-failure /
  `ZAP_GC_HEAP_LIMIT`-cap paths, which it unifies. Robustness only — it makes an
  OOM diagnosable; it does not change any GC-state behavior and is independent of
  the #223 deadlock fix.
- **gc-boehm async-preempt vs stop-the-world: the `g_boehm_in_stw` guard is now
  a correctly-ordered atomic** (#693, D-0120; folds in #213 for this flag). Under
  the conservative collector the SIGURG fairness-preemption handler performs an
  in-signal coroutine switch (`copreempt`) gated by `g_boehm_in_stw`, which
  brackets bdwgc's stop-the-world. The flag was a plain `volatile sig_atomic_t`,
  which carries **no cross-thread ordering**: on a weak-memory arch (aarch64) the
  collector's "STW starting" store and the handler's load have no happens-before,
  so the handler could read a **stale 0 after STW had already begun** and switch a
  thread out from under the collector — the memory-ordering half of a latent
  TOCTOU. The flag is now accessed only via `__atomic` seq-cst: a release-ordered
  store around the STW bracket (`zap_boehm_gc_event`) paired with acquire loads in
  the preempt handler and the SIGPROF sampler, so once stop-the-world is announced
  no later handler can observe it unset. The handler also reads the guard **last**,
  after its cheap SP/`no_preempt` checks, minimizing the check→act window. The
  residual window (STW begins after the handler's load but before its swap
  completes) is left as a benign fairness race, not a correctness lock: a switch
  that races STW is GC-root-safe by the same invariant that makes the lock-free
  switch-path bottom update safe (D-0117) — registers are captured by bdwgc's STW
  register snapshot and the green stack is scanned as heap via its `G`. POSIX
  deliberately does **not** mirror Windows' `g_preempt_stw_mu` across the swap
  (`pthread_mutex_lock` is not async-signal-safe). This hardens a real latent
  ordering hazard; note the originally-reported #295 flake (`threading_counter`
  under Boehm) was **never confirmed to reproduce** and now passes 2000× on
  aarch64 after D-0117, so definitive confirmation needs an x86 gc-boehm loop
  under load (CI/a box), not a local run. New walled-garden golden
  `gc_preempt_stw_stress` exercises the preempt-during-STW overlap under both
  collectors.
- **aarch64 multicore GC deadlock: the scheduler's stack-bottom update is now
  lock-free** (#223, D-0117). The M:N scheduler retargets the conservative
  collector's stack bottom on every context switch (`switch_to_g` →
  `set_bottom`); it used to do so under the bdwgc **global allocator lock**,
  which is also held inside `GC_MALLOC`. A coroutine descheduled while briefly
  holding that lock could then never be resumed — resuming any coroutine demanded
  the same lock — deadlocking the whole scheduler (confirmed by gdb on a 32-vCPU
  Graviton box; masked on x86 and small CI runners). `set_bottom` now calls
  `GC_set_stackbottom` directly (a single pointer store to this thread's own
  stack cold-end), so the switch path takes no global GC lock and the scheduler
  can always resume the lock-holder. Collector-agnostic: a no-op under gc-immix
  (the shim/`GC_call_with_alloc_lock` were already no-ops). Note: this fixes the
  gdb-confirmed lock-ordering deadlock; box-level confirmation on the high-core
  Graviton host (#650) is pending. The three `.skip_arch` aarch64 stress-golden
  quarantines it introduced (PR #445) have since been removed (#698, D-0121) now
  that the deadlock is fixed.
- **`dict.get` (and other receiver-type-driven specializations) on a
  module-level dict global** (#671, D-0112). Monomorphization now seeds
  explicitly-typed module-level globals (epic #661: `const` #658, mutable #662)
  into each function body's type environment, so a method call whose receiver is
  a bare module-global name resolves the receiver's concrete type exactly as it
  does for a local. Previously `t.get(k)` / `t.get(k, default)` on a module-global
  dict fell through IR lowering as a bare `get` and was rejected with the generic
  `ZAP6001` umbrella (`dict.get() takes no arguments`), while the identical call
  on a *local* dict worked. The fix is general — any receiver-type-driven
  specialization (`dict.get`'s `Optional[V]`/`get_default` rewrites, generic-method
  dispatch) now fires on a module global — and a shadowing local of the same name
  still takes precedence (static lexical scoping). `auto`-typed module globals,
  whose type mono does not re-derive, keep the prior opaque-receiver behavior.
- **Module-level globals of a user *generic* type** (#690, epic #661 / D-0112).
  Monomorphization now rewrites module-level global bindings (`Item::Stmt`) the
  same way it already rewrites a local `Stmt::Let`: it requests the global's
  declared generic type as a concrete instance (`Box[int64]` → `Box$int64`) and
  monomorphizes its initializer. Previously a top-level `Box[int64] g = Box(7)`
  passed through mono unchanged, so the instance was never emitted and lowering
  failed with `type 'Box' is not supported by codegen yet` / `unknown variable
  'g'`, while the identical binding as a *local* worked. Const and mutable
  generic globals, distinct instantiations as separate globals, and method calls
  on them all resolve now.
- **Member access on a user struct whose name collides with a prelude type**
  (#689, epic #661). A user type named after a prelude type (e.g. `Counter`,
  which the `concurrent` prelude also defines) is still shadowed by the user's,
  but a prelude *helper function* that references the shadowed type — the
  concrete `__concurrent_counter`, whose body builds `Counter(_cell=Atomic(…))` —
  was injected regardless and bound its `._cell` access to the user's unrelated
  struct, failing at lowering with `no field '_cell' on struct` (for both a
  field read and a method call, and for a *local* as well as a module global).
  `with_prelude` now drops a prelude helper that mentions a user-shadowed prelude
  type, so such member access resolves against the user's struct. Programs with
  no such collision are byte-for-byte unchanged, and `concurrent.Counter(…)`
  itself still lowers when the user does not redefine `Counter`. (Non-colliding
  struct-typed module globals were never affected.)
- Preserve `bytes` result types for `crypto.digest`, `crypto.hmac_sha256_raw`,
  and `crypto.random_bytes` during IR lowering (#647), fixing binary indexing,
  slicing, concatenation, and equality on inferred locals. Hex/text APIs and
  opaque streaming handles retain their existing representation. Centralize
  crypto result typing in one explicit table and share call emission, including
  integer count coercion, RSA exception handling, and boolean normalization.

### Security

- Bind RSA-named signing and verification to ordinary RSA keys and explicit
  PKCS#1 v1.5 padding (#642). Previously generic EVP dispatch could sign with
  an EC/DSA key or verify a foreign-family signature. Wrong-family signing
  raises `ValueError`; verification returns `false`, including RSA-PSS keys.
  Emit sign calls through the throwing-call path so local handlers catch
  parsing, key-family, and signing errors.

- Preserve the RSA sign result as `bytes` during IR lowering (#644), fixing
  byte operations on inferred signature locals without changing the runtime ABI.
- Share RSA key-family/padding enforcement between signing and verification;
  emit DEBUG diagnostics for verifier key/configuration errors through the
  existing logging configuration, without including cryptographic inputs.

[Unreleased]: https://github.com/zaplang-io/zap/commits/main
