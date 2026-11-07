# Best practices for official non-core Zap packages

Guidance for the official, Zap-maintained libraries that live outside this
repository — `jwt`, `jsonschema`, `redis`, `templates`, `validation`, the
`aws`/`requests`/`fastzap` families, and future additions. These packages are
not part of the core language or standard library, but they ship under the
`zaplang-io` org and carry the project's name, so they are held to a higher bar
than a typical third-party crate.

This document was distilled from full security and correctness reviews of the
`main` branch of several of these packages. It is a **checklist and a rationale**,
not a rulebook: deviate when the domain demands it, but record the deviation.

> Scope note. The hard rules about issues/PRs, one-issue-one-PR, rebasing,
> agent attribution, and the **golden rule for core defects** live in the
> workspace `AGENTS.md` and are not repeated here. This document is about how to
> design, harden, test, document, and ship the library code itself.

---

## 1. State a threat model, in writing, first

Every package must answer one question in its `README.md` (or `SUPPORT.md`)
before it ships: **what is trusted, and what is attacker-controlled?** Every
later decision — which inputs to bound, what to escape, what to reject — follows
from that answer, and a reviewer cannot judge the library without it.

Worked examples from the existing packages:

- **jwt** — the token (header, payload, signature) is fully attacker-controlled;
  keys and the configured algorithm are trusted.
- **jsonschema** — *both* the schema and the instance document are untrusted.
- **redis** — the application supplies command arguments (often derived from
  end-user input); the server's responses are parsed by the client over a
  possibly-untrusted network.
- **templates** — the *data* passed to a template is untrusted (and must be
  safe); the *template text itself* is trusted (author-controlled), and that is
  stated as an explicit non-goal of sandboxing.

Write the threat model down even when the answer is "templates are trusted." The
value is in making the boundary explicit so users know where their
responsibility begins.

---

## 2. Resource limits: bound depth **and breadth and total work**

This is the single most important and most frequently incomplete area.

Bounding recursion depth is necessary but **not sufficient**. The common failure
is to guard nesting depth and stop there, leaving an input *breadth* or
*total-work* dimension unbounded — which is still a denial-of-service vector when
the input is untrusted.

Require all of the following, as applicable:

- **Recursion / nesting depth** — a hard numeric cap on every recursive descent
  (parse nesting, evaluation nesting, object/array nesting), checked *before*
  recursing so native stack exhaustion is impossible. (jwt caps JSON nesting at
  32; jsonschema and templates cap at 64–128 across their recursive entry points.)
- **Input breadth** — a cap, or a global work budget, on array length, property
  count, member count, and iteration counts. *Depth guards do not catch
  quadratic-in-length work.* A real finding: `jsonschema`'s `uniqueItems`/`enum`
  run O(n²) deep-equality over an attacker-sized array with no length cap, so a
  100k-element array is a CPU-exhaustion request against a very common schema.
- **A single monotonic "work" / "step" budget** threaded through the hot path, so
  that compounding constructs (nested loops, macro expansion, repeated
  comparisons) draw down one shared counter and cannot multiply around a
  per-operation limit. `templates` does this well: one 100,000-step counter is
  decremented per node, per eval, per loop item, and per character in string
  ops, which is what contains billion-laughs-style expansion.
- **Absolute size caps** — maximum input size, maximum output size, maximum
  decoded segment size, maximum buffered response. (jwt caps the token at 50 000
  bytes and each decoded segment; redis bounds every RESP frame by `max_frame`.)
- **Overflow-safe length arithmetic.** When you read a length field off the wire
  or compute a budget, write the arithmetic so it cannot wrap. redis is the
  reference here: it accumulates numbers negatively with explicit last-digit
  guards (so `INT64_MIN` is representable without wrapping) and phrases every
  limit check as `count > limit - pos - 2` rather than `pos + count > limit`.

If a dimension is genuinely unbounded by design, **say so in the docs** and tell
the operator to bound it upstream (size-limit the request, set a process
wall-clock/memory limit). An undocumented unbounded dimension is a latent DoS;
a documented one is a deployment contract.

---

## 3. Parse defensively; authenticate before you trust

- **Reject malformed input early and specifically.** Accept only the canonical
  form and reject everything else rather than being liberal. jwt enforces
  canonical, unpadded base64url and rejects duplicate JSON members at every
  depth; redis accepts only the five RESP type tags and rejects bare-LF, lone-CR,
  unknown tags, and streamed encodings outright.
- **Authenticate before parsing untrusted structure.** jwt verifies the
  signature *before* base64-decoding and JSON-parsing the payload — only the
  header (needed to pick the algorithm) is parsed first, under strict caps. Do
  the minimum parsing needed to authenticate, authenticate, then parse the rest.
- **Frame, don't concatenate.** When building a protocol message, use
  length-prefixed framing so that control characters inside a field cannot inject
  structure. redis builds every command as a length-prefixed RESP bulk-string
  array, so CRLF/space/NUL inside an argument is inert — there is no
  string-concatenation command path anywhere. Never build wire protocol by string
  interpolation of untrusted values.
- **Re-validate what the server tells you.** redis validates that redirect slots
  match the key's computed slot, that returned keys belong to the requested set,
  and that a transaction's `EXEC` reply length equals the queued command count. A
  response from the network is untrusted input too.

---

## 4. Choose secure defaults, and don't offer an easy way to turn them off

- Default to the safe behavior: autoescaping **on** (templates), signature
  verification **on**, required-claims like `exp` **on** (jwt).
- Pin, don't infer, security-critical choices. jwt requires the caller to pass
  the algorithm and rejects a token whose header disagrees — this is what blocks
  the `alg`-confusion and `alg:none` attack families. Never read the algorithm (or
  any other trust decision) *from* the attacker-controlled input.
- Do not expose an insecure escape hatch casually. redis has **no**
  skip-TLS-verification option and never silently falls back from TLS to
  plaintext. If an insecure mode is unavoidable, make it loud, explicit, and
  documented as dangerous.
- Bind related parameters together. jwt binds key family ↔ algorithm ↔ curve ↔
  key size ↔ private/public role, so a key can only be used the one way it is
  meant to be.

---

## 5. Keep untrusted data away from host objects

Libraries that evaluate expressions or traverse user data over an in-language
value model (not host-language objects) get a strong sandbox for free. Both
`templates` and `jsonschema` operate purely over `JsonValue`, so the entire
reflection/attribute-pivot family (`__class__`/`__globals__`-style escapes) is
structurally impossible — there is no bridge from data to code.

If you build an evaluator, interpreter, or query/filter engine:

- Operate over a closed value type; never expose host attributes, methods, or
  module globals to the evaluated input.
- Make callables un-forgeable from data (templates: a value's `callable` slot
  defaults to "not callable", and only real macros can be invoked).
- Make filters/functions a fixed allowlist, validated at compile time, with no
  member that does I/O, `eval`, or marks attacker data trusted.

---

## 6. Errors: pick return-vs-raise deliberately, and never leak secrets

- **Decide and document whether a given failure returns or raises.** Zap's
  convention is string exceptions raised as `raise "Prefix: message"` and matched
  with `e.startswith("Prefix:")`. That is fine, but be deliberate about which
  conditions throw. A real ergonomics trap: `jsonschema` *raises* when an
  untrusted instance exceeds a limit, so a caller validating untrusted input must
  wrap every entry point or a crafted input becomes an uncaught crash. If callers
  will routinely hit a condition on untrusted input, consider returning a result
  instead of throwing — and if you throw, document it prominently on the public API.
- **Never put secrets or raw wire bytes in an error message.** redis raises only
  the *stage* name on an auth failure (`"…AUTH"`), never the password or the
  server's text, and masks credentials in its debug/`describe` output. Audit every
  error path and every logging/debug helper for credential or token leakage.
- Weigh specificity against leakage. Collapsing many internal errors into one
  generic error (as jwt's JWK loader does) is defensible for a loader but costs
  debuggability; a security-sensitive parser should lean toward "reject with a
  category" over "echo what was wrong with the attacker's input."

---

## 7. Cryptography and transport

- **Use the stdlib's verified primitives; never hand-roll crypto or ASN.1.** This
  is also the golden rule in action — if the primitive you need is missing from
  core, report it in `zaplang-io/zap` and keep the dependent work blocked rather
  than rolling your own.
- Keep private and public key material separate in the API, and bind each key to
  its one algorithm/curve.
- For transport, require certificate-chain and hostname verification by default,
  with no silent plaintext fallback (redis).
- Constant-time comparison: if you compare secrets or MACs and the stdlib has no
  constant-time primitive yet, **document the limitation honestly** rather than
  claiming a guarantee you can't make, and file the core request. (jwt documents
  that its HMAC comparison is not guaranteed constant-time and tracks the core
  primitive in `zaplang-io/zap#729`.)

---

## 8. Concurrency: document the thread-safety contract

Several packages carry mutable scratch state on a reusable object — `jsonschema`'s
compiled `Validator` (evaluation marks/counters) and `templates`' `Environment`.
That is fine, but it means the object is **not** safe to share across concurrent
callers, and that is exactly the kind of thing a user gets wrong silently.

State the contract in the README: which objects are immutable and shareable,
which must be one-per-caller, and whether the public AST/structs may be mutated.
Make the safe pattern the obvious one.

---

## 9. Match the Python equivalent, and log the divergences

Per `AGENTS.md`, a non-core library's surface should match the equivalent popular
Python library as closely as the language allows — PyJWT for `jwt`, `jsonschema`
for `jsonschema`, redis-py for `redis`, Jinja2 for `templates`, pydantic for
`validation`. This lowers the learning cost and lets users port code.

Where Zap forces a divergence (numeric model, string semantics, no keyword
args, etc.), **record it** — in a `DECISIONS.md` entry for a design choice, and in
`SUPPORT.md`/`README.md` as a user-facing "differences from X" note. The existing
packages do this well: documented divergences include ASCII-only casing,
binary64 rounding, byte-vs-codepoint string length, and offline-only `$ref`
resolution.

---

## 10. Tests are intent documentation — make them exhaustive

Per `AGENTS.md`, tests build a walled garden around the code; they are never
minimal. For these packages specifically:

- **Independent cross-implementation vectors, with provenance.** Generate test
  vectors with a *different* implementation (OpenSSL for jwt's asymmetric
  algorithms, Python stdlib for HMAC, Jinja 3.1.6 for templates' 246 differential
  fixtures) and record where they came from and at what pinned version. Isolate
  any generator (e.g. a Python+OpenSSL script) so that consumers of the library
  never depend on it.
- **Authoritative known-answer tests.** Where an RFC or spec publishes a vector,
  pin it verbatim (jwt's RFC 7515 A.1 KAT). Where a conformance suite exists, pin
  it to an upstream commit and track pass counts in CI (jsonschema pins the
  JSON-Schema-Test-Suite and is candid about its documented skips).
- **An abuse / regression corpus.** A dedicated suite of malformed, tampered,
  confused, and oversized inputs that asserts each is rejected the intended way —
  this is what keeps a future PR from quietly weakening a guarantee.
- **Breadth and complexity regression tests.** Explicitly test the resource
  bounds from §2, including a *large-breadth* input (a big array, a many-node
  schema) with a timing or work-count assertion. A review of jsonschema found its
  depth/regex bounds were tested but its breadth bounds (the actual DoS vector)
  were not — close that gap.
- **Durability under GC / repetition.** Repeated sign/verify or
  encode/render loops that allocate fresh objects, to shake out GC and
  steady-state bugs (jwt's stress test).
- **A package-consumer / import test and a publish dry-run** in CI, so that the
  library actually imports and installs as an external dependency, not just as an
  in-repo test.

---

## 11. CI hardening

Every package's CI should, at minimum:

- **Pin the Zap toolchain by immutable image digest** (`ghcr.io/zaplang-io/zap@sha256:…`),
  not by a mutable tag. This is how jwt, jsonschema, and redis pin, and it is
  what makes a green build reproducible and a toolchain regression diagnosable.
- **Pin GitHub Actions by commit SHA**, not by tag.
- Set least-privilege `permissions:` (e.g. `contents: read`).
- Set a per-job timeout, and wrap long conformance/integration runs in an
  explicit `timeout`.
- Run `zap fmt --check`, the unit tests, and the package-consumer test on every PR.
- Keep the toolchain pin current: when a dependent primitive lands in core, bump
  the digest to the published image that carries it (and note why in the PR).

---

## 12. Documentation set

Non-core libraries may skip the full core documentation set, but anything slated
for public release should carry:

- **`README.md`** — what it is, the Python-equivalent it tracks, a quickstart,
  the **threat model**, every **resource bound**, and every **security caveat**
  (see below). Be candid; understated limits are the ones that bite.
- **`CHANGELOG.md`** — notable changes, per Keep a Changelog / SemVer.
- **`DECISIONS.md`** — start it as soon as the first non-obvious design choice is
  made (numeric model, which subset of a protocol/spec is supported, return-vs-raise,
  divergences from the Python equivalent). Do not wait for release.
- **`SUPPORT.md`** (or a README section) — the supported surface, the explicit
  non-goals, and the known divergences.

Security caveats deserve their own prominent callouts, because the existing
reviews show they are easy to leave implicit. Real examples worth stating loudly:

- "This is not a security sandbox; use trusted template sources." (templates)
- "Never pass untrusted data as an `include`/`extends`/`import` name" — a dynamic
  template name from request data can disclose any file under the loader root.
  (templates)
- "Escaping is HTML-only; `{{ x }}` in a `javascript:` URL, a `<script>` body, or
  an unquoted attribute is not XSS-safe." (templates)
- "Validator/Environment objects carry mutable scratch and are not safe to share
  across threads." (jsonschema, templates)
- "Signature comparison is not guaranteed constant-time." (jwt)
- "Hostname dials without a resolved IP use an unbounded synchronous resolver;
  bounded dialing requires `resolved_ip`." (redis)

---

## 13. Vendored data and provenance

If the package embeds data copied from elsewhere (meta-schemas, test corpora,
fixtures), vendor it **unmodified**, at a **pinned upstream commit**, with its
`LICENSE`, and generate any in-repo embedding of it from a checked-in tool. Keep
the provenance recorded (jsonschema's `vendor/metaschemas` is copied from
`python-jsonschema/jsonschema-specifications` at a pinned commit; redis and jwt
regenerate their corpora from pinned upstream tools).

---

## 14. Relied-upon core contracts

When a library *depends on* a core/stdlib behavior that is not obviously
guaranteed — not a defect, but a contract — note it explicitly and keep a test or
a doc line that pins the expectation, so a future core change that breaks it is
caught. Examples surfaced in review:

- redis sets deadlines with `net.set_deadline_ns` and relies on TLS sockets
  honoring them; it documents and tests this as a supported contract.
- Several libraries work around `len(str)` counting UTF-8 bytes while slicing
  uses codepoints; the ones that handle Unicode do their own codepoint counting.

Per the golden rule in `AGENTS.md`: if the contract is actually broken, or the
primitive you need is missing, **report it in `zaplang-io/zap` and stop** — do not
work around a core defect to ship library functionality, and do not implement the
core fix unless explicitly assigned.

---

## Quick checklist

- [ ] Threat model written down (what's trusted vs attacker-controlled).
- [ ] Recursion depth bounded on every recursive path, checked before recursing.
- [ ] Input **breadth** (array/collection length, member/iteration counts) bounded or on a global work budget.
- [ ] A single monotonic work/step budget through the hot path.
- [ ] Absolute size caps on input, output, and buffered data.
- [ ] Overflow-safe length/budget arithmetic.
- [ ] Malformed input rejected early; only canonical forms accepted.
- [ ] Authenticate before parsing untrusted structure.
- [ ] Protocol messages length-framed, never string-concatenated from untrusted values.
- [ ] Secure defaults (escaping/verification on); trust decisions never read from attacker input; no casual insecure escape hatch.
- [ ] Untrusted data confined to a closed value model; no host-object/reflection reach.
- [ ] Return-vs-raise chosen deliberately and documented; no secrets in errors or debug output.
- [ ] Stdlib crypto/TLS only; verification on by default; constant-time gaps documented.
- [ ] Thread-safety contract documented.
- [ ] API tracks the Python equivalent; divergences logged in DECISIONS.md and SUPPORT.md.
- [ ] Tests: independent cross-impl vectors (with provenance), KAT/conformance pinned to upstream commit, abuse corpus, **breadth/complexity regression tests**, GC/stress durability, consumer-import + publish dry-run.
- [ ] CI: toolchain pinned by digest, actions pinned by SHA, least-privilege permissions, per-job + long-run timeouts, `zap fmt --check`.
- [ ] Docs: README (threat model + every bound + every security caveat), CHANGELOG, DECISIONS, SUPPORT.
- [ ] Vendored data unmodified, pinned, licensed, with recorded provenance.
- [ ] Relied-upon core contracts noted; genuine core gaps reported upstream, not worked around.
