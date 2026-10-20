# Proposal: Official `aws` SDK — botocore-model code generation

- **Author(s):** Claude (design session for I#89)
- **Last updated:** 2026-10-20
- **Status:** Under review (recommendations below; the Option[T] prioritization
  call is being made at the orchestrator level — this doc is written to stand
  either way).
- **Discussion:** I#89 (Official AWS package, lambda first). Depends on/coordinates
  with I#69 (pkg manager), I#79/I#100 (`json`), I#105 (`crypto`), I#104 (`net`/
  `http`), I#147 (`tls`), I#80 (netpoller), D-0017 (user generics, staged).

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

An official `aws` SDK for Zap, distributed as managed packages under `zaplang-io`
(the official-packages tier, D-0056 — **not** bundled std modules).
boto3/botocore is **data-driven**: ~400 services are described entirely by JSON
service models (`botocore/data/<svc>/<version>/service-2.json`), and boto3 builds
clients from them *at runtime* via Python reflection.
Zap is AOT-compiled and statically typed with **no runtime reflection**, so the
boto3 runtime-generation model does not port.
The Zap-idiomatic equivalent keeps the data-driven *insight* but moves it to
**build time**: a generator consumes the same Apache-2.0 botocore models and emits
**static typed Zap source** (per-operation input/output structs via
`@derive(Json)`, a `Client` with one method per operation, typed errors,
paginator configs), layered on a hand-written **core** (SigV4 signing, credential
chain, endpoint resolution, HTTP transport, retries, protocol serializers).
This is the approach aws-sdk-rust / aws-sdk-go-v2 / aws-sdk-js-v3 take.
Two structural wins over boto3 fall out of being static: you compile/download
**only the services you use** (per-service packages), and the linker drops
**unused operations** (dead-code elimination), so binaries stay light and Lambda
cold starts stay in the Rust/Go class.

The design is bounded by one load-bearing language gap — `Option[T]` (user
generics, D-0017, staged) — which governs how faithfully optional fields can be
modeled.
This doc presents the preferred path (land `Option[T]`) **and** a fallback that
ships read-oriented services first without it, so the recommendation is correct
regardless of how that call lands.

## Background

I#89 scopes an official AWS package, **lambda-first**, in the official-packages
tier (D-0056): developed by us but imported/managed by the package manager (I#69),
with nothing AWS-specific in the default modules.
It distinguishes two separately-valuable deliverables that this doc keeps
distinct:

- **(a) Running Zap *as* a Lambda** — a custom-runtime loop that polls
  `GET $AWS_LAMBDA_RUNTIME_API/.../invocation/next`, dispatches a user handler,
  and `POST`s `.../response` or `.../error`.
  Needs only `http` + `json` + env-var credentials.
  **Does not need SigV4 or the SDK.**
- **(b) The SDK that *calls* AWS services** (this doc's main subject) — needs
  signing, credential resolution, transport, retries, protocol codecs, and the
  generated per-service clients.

What the language/stdlib already provide (all landed) and the design builds on:

- **`json` / `@derive(Json)`** (I#100): compiler-built-in derive generating
  `from_json`/`to_json` + `__*_json_value__` dunders over structs/enums; strict
  typed decode, lenient on unknown keys; `@json(name=...)` field rename; `@default`
  (the `=` form) makes a field optional.
  Payload-free `enum` ↔ JSON string; data-carrying `enum` ↔ internally-tagged
  object.
  This is the request/response (de)serialization substrate for JSON-protocol
  services.
- **`crypto`** (I#105): `crypto.sha256(data) -> str`, `crypto.hmac_sha256(key,
  msg) -> str` (hex), streaming hash handles, `crypto.random_bytes`.
  SHA-256 + HMAC-SHA256 are exactly the SigV4 primitives — with one gap noted
  under Dependencies.
- **`net` / `http`** (I#104) + **`tls`** (I#147) on the **netpoller** (I#80):
  `http.get/post/request` now do `https://` with certificate + hostname
  verification ON by default, over the M:N goroutine scheduler.
  Client currently sends `Connection: close` and reads to EOF — **keep-alive /
  connection pooling is deferred** (noted under Dependencies).
- **`datetime`** (I#99): UTC `DateTime` value struct with fields year..second —
  enough to format the SigV4 `X-Amz-Date` (`YYYYMMDD'T'HHMMSS'Z'`) and credential-
  scope date (`YYYYMMDD`).
- **Goroutines / structured concurrency** (I#15/I#50) and the **package manager +
  zenv** (I#69) for distribution.

What is **staged, not landed**, and therefore shapes the recommendation:

- **User generics (D-0017).**
  So a proper `Option[T]` for "field may be absent" is **not expressible yet**.
  AWS shapes are overwhelmingly optional, so this is the single biggest constraint
  — larger than any missing module (see §3.1).
- **Typed exception classes (I#12).**
  `raise` carries a string today; typed service-error classes are not first-class.
  Same constraint `json` hit; the SDK inherits its stopgap (§7).
- **No `xml` module.**
  Gates the `rest-xml` (S3, Route53, CloudFront) and `query`/`ec2` (STS, SNS, IAM,
  EC2) protocols (§4).

## Proposal

### 1. Generation model (fork G) — build-time codegen

boto3 reads `service-2.json` on import and synthesizes client classes with Python
reflection.
Zap cannot: it is AOT with no runtime class generation.
Three options:

- **G1 — build-time codegen → static Zap *(recommended)*.**
  A generator reads the botocore models once (at package-build time, by us) and
  emits static `.zap` source.
  Same data-driven philosophy as boto3; codegen instead of reflection.
  Typed, lean, native — matches aws-sdk-rust/go-v2/js-v3.
- **G2 — runtime-depend on CPython botocore via pyinterop (I#27).**
  Mechanically possible, **rejected**: it drags a CPython runtime + boto3 into
  every binary, destroying I#89's native-cold-start selling point, and returns
  untyped `dict`s — discarding Zap's static-typing pitch.
  Legitimate *dev-time* use only: botocore as a **test oracle** (diff our SigV4 /
  serialization against botocore's).
- **G3 — dynamic/untyped runtime client** that loads `service-2.json` at runtime
  and offers `client.call("dynamodb", "GetItem", value) -> json.Value`.
  Needs no reflection (data-driven *dispatch*, not codegen), so it is possible in
  Zap, and it is closest to boto3's spirit — but it discards static typing and
  still needs the whole hand-written core.
  **Deferred** as an optional opt-in escape hatch in its own module (see §9 — it
  must not be in the default typed client, or it defeats DCE).

*Recommendation: G1. G3 later as an opt-in module; G2 only as a test oracle.*

### 2. What is generated vs the hand-written core

The models describe the **what**; they contain **none** of the runtime **how**.
botocore has the same split (`data/` + a Python runtime).

**Hand-written once — `aws-core` package:**

- SigV4 signer (§5) + presigner.
- Credential provider chain (§5).
- Endpoint/region resolution (§6) — simple template v1, ruleset engine deferred.
- HTTP transport wrapper over `http`/`tls`/netpoller (§6).
- Retry policy — standard mode, backoff + jitter (§6).
- Protocol serializers/parsers — one per wire protocol (§4).
- Pagination + (later) waiter runtime engines (§7).
- Error base types (§7).

**Generated per service (from the models):**

- Input/output **structs** with `@derive(Json)` and `@json(name=...)` wire names.
- A **`Client`** with one **method per operation**, each assembling the request
  (protocol, target/uri, bindings) via core helpers and decoding the response.
- Typed **error** enums per operation (§7).
- **Paginator** configs (input/output tokens, result key) from `paginators-1.json`
  (§7).
- **Waiter** acceptors from `waiters-2.json` (deferred, §7).

The generator consumes a per-service **bundle**: `service-2.json` (shapes +
operations + `metadata.protocol`), `paginators-1.json`, `waiters-2.json`, and
endpoint rules — all Apache-2.0, vendored into the service package.

### 3. Model → Zap type mapping

The mechanical core:

| botocore shape | Zap type |
|---|---|
| `string` | `str` |
| `string` + `enum:[…]` | generated `enum` + `Unknown(str)` (§3.2) |
| `integer` / `long` | `i64` |
| `float` / `double` | `f64` |
| `boolean` | `bool` |
| `timestamp` | `DateTime` + format tag (§3.3) |
| `blob` | `bytes`; `streaming:true` → reader (§3.3) |
| `structure` | `struct` + `@derive(Json)` |
| `structure` + `exception:true` | generated error (§7) |
| `list{member}` | `list[T]` |
| `map{key,value}` | `dict[str, V]` |

Recursive shapes are fine and sometimes ideal: DynamoDB's `AttributeValue` is a
recursive tagged union → a data-carrying `enum`, exactly like `json.Value`.
Naming: members are PascalCase (`TableName`) → snake_case fields (`table_name`)
with `@json(name=...)` set to the `locationName` trait if present, else the member
name; operations `GetItem` → `get_item`; Zap keywords escaped deterministically
(`type` → `type_`).

The non-mechanical parts are three, below.

#### 3.1 Optionality / absent-vs-zero (fork O — the load-bearing one)

AWS shapes are overwhelmingly optional; most output members and many input
members may be absent.
The clean model is `Option[T]` (why aws-sdk-rust wraps nearly every field), but
that needs user generics (D-0017, **staged**).
Without it, `json`'s answer is `@default` → a missing key uses a zero default,
which **conflates "absent" with "present-but-zero."**
That is tolerable for read-oriented calls but **wrong for mutating calls** (sending
a zero you meant to omit changes the request).

- **O-a — land `Option[T]`, model optional as `Option[T]` *(preferred)*.**
  Cleanest and correct for all operations; couples the SDK's full scope to D-0017.
- **O-b — v1 uses `@default`/zero**, and the **first wave is restricted to
  read-oriented operations** where absent≈zero does not corrupt requests.
  Ships today with no language change.
- **O-c — generated presence flags** (`has_table_name: bool` alongside each
  optional field, protobuf-v2 "hazzer" style); the serializer omits unset fields.
  Works today without generics; costs ergonomics and generated bloat.

*Recommendation: O-a if `Option[T]` is scheduled; otherwise O-b for the first
wave, with O-c available as a targeted patch for specific mutating operations that
cannot wait.*
The first-wave service list for O-b is in §8.

#### 3.2 Open enums (fork E)

AWS enums are **open** — the service may return a value the model never listed.
A closed Zap `enum` would fail to decode the day AWS ships a new value.
Generate the modeled variants **plus a data-carrying `Unknown(str)` catch-all**:

```python
enum TableStatus:
    Active            # "ACTIVE"
    Creating          # "CREATING"
    Unknown(str raw)  # any unmodeled value — never fails to decode
```

*Recommendation (E):* open enum with `Unknown(str)`.
Needs one `json` affordance: decoding an unrecognized variant name into
`Unknown(s)` rather than erroring (a derive option) — see Dependencies.

#### 3.3 Timestamps / blobs / streaming (fork Ts)

- **Timestamp** wire format depends on protocol **and** location: awsJson default =
  epoch seconds (number); rest-xml/query default = ISO8601; HTTP headers =
  RFC822/http-date; a per-member `timestampFormat` trait overrides.
  The generator resolves each field's effective format and stamps it; the codec
  applies it.
- **Blob** = base64 string in JSON/XML bodies → `bytes`; `streaming:true` (e.g. S3
  `GetObject` body) → a **reader/stream**, not materialized `bytes`.

*Recommendation (Ts):* the generator maps types with format/blob tags and
`@derive(Json)` grows **custom-field-codec hooks** (`@json(ts="epoch"|"iso8601"|
"rfc822")`, base64 blob) — an explicit ask on the `json` module (Dependencies).

### 4. Wire protocols & first-wave coverage (fork P)

`metadata.protocol` selects the serializer/parser:

| protocol | example services | v1? |
|---|---|---|
| `json` (awsJson 1.0/1.1) | DynamoDB, SQS (2023+) | **yes** |
| `rest-json` (restJson1) | Lambda control plane/invoke, API GW | **yes** |
| `rest-xml` | **S3**, Route53, CloudFront | deferred (needs `xml`) |
| `query` / `ec2` | STS, SNS, IAM, EC2 | deferred (needs `xml`) |

For **awsJson** the HTTP body *is* the input/output shape → `to_json`/`from_json`
do everything and the generated per-op code is a one-liner.
For **rest-json** outputs/inputs scatter across `location` traits — `uri` (path
templating `{Bucket}`), `querystring`, `header`, `headers` (map→prefix),
`statusCode`, and `payload` (the single member that *is* the body); `from_json`
covers only the body-bound slice, the core routes the rest.
`rest-xml`/`query` additionally need XML (de)serialization the stdlib lacks.

*Recommendation (P):* v1 ships **JSON-family protocols only** (awsJson +
rest-json).
`rest-xml` + `query` (hence S3, STS, SNS, IAM, EC2) wait on an `xml` module — its
own stdlib/pkg ticket and the main coverage gate after Option[T].

### 5. Auth / request signing (fork A)

**SigV4** on `crypto`.
The canonical-request → string-to-sign → signing-key chain is HMAC-SHA256 + SHA-256,
both present in `crypto`.
The signing key is a keyed chain —
`kDate = HMAC("AWS4"+secret, date)`, `kRegion = HMAC(kDate, region)`,
`kService = HMAC(kRegion, service)`, `kSigning = HMAC(kService, "aws4_request")`
— where **each step feeds the previous step's raw bytes as the next key.**
`crypto.hmac_sha256` returns a **hex string** today; the chain needs **raw-bytes**
output.
This is the one hard crypto gap (Dependencies).

**SigV4A** (asymmetric, ECDSA P-256; multi-region access points) needs EC crypto
not in `crypto` — **deferred**.

**Credential provider chain — v1 recommendation (fork A-creds):**

1. **Environment** (`AWS_ACCESS_KEY_ID`/`SECRET_ACCESS_KEY`/`SESSION_TOKEN`) — also
   covers Lambda, which injects these.
2. **Shared config/credentials files** (`~/.aws/credentials`, `~/.aws/config`,
   `AWS_PROFILE`) — static keys only; needs a small INI parse.
3. **IMDSv2** (EC2 instance metadata; token `PUT` then `GET`, JSON creds).
4. **Container/ECS** (`AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`/`FULL_URI`, JSON).

**Deferred:** STS-based providers (AssumeRole, AssumeRoleWithWebIdentity), SSO, and
`credential_process` — all need the `query`/STS path (XML) or extra flows.
Resolution tries sources in order; the chain is in `aws-core`.

*Recommendation (A):* SigV4 v1 (SigV4A deferred); credential chain = env + shared-
file-static + IMDSv2 + container (STS/SSO/web-identity deferred with the `xml`/
`query` work).

### 6. Transport, concurrency, retries (fork X)

- **Transport:** HTTPS over `http` + `tls` on the netpoller (verify ON).
  Each service call parks on the netpoller; many calls run concurrently as
  goroutines.
- **Connection reuse:** `http` v1 sends `Connection: close` and reads to EOF —
  **no keep-alive/pooling yet**.
  v1 SDK is correct but one-request-per-connection; connection pooling is a
  fast-follow gated on `http` keep-alive (Dependencies).
- **Retries (standard mode):** up to 3 attempts (2 retries); exponential backoff
  with **full jitter**; retry on throttling (`Throttling*`,
  `TooManyRequestsException`, HTTP 429/503), transient 5xx, and connection errors;
  clock-skew correction on signing errors.
  **Deferred:** adaptive mode and the client-side retry-quota token bucket.

*Recommendation (X):* http+tls+netpoller transport, goroutine concurrency,
standard-mode backoff+jitter in v1; pooling + adaptive retries fast-follow.

### 7. Ergonomics boto3 users expect (fork Erg)

- **Per-operation input/output types** — generated; core of the typed surface. **v1.**
- **Paginators** — generated config (input token, output token, result key) driving
  an `Iterator` over pages (reuses D-0048). High value, small runtime. **v1.**
- **Presigned URLs** — query-string SigV4; cheap once the signer exists, useful even
  before S3. **v1 (in the signer).**
- **Errors** — AWS responses carry an error **code** + message (+ requestId, HTTP
  status).
  Typed per-error classes want typed exceptions (I#12, staged), so v1 uses `json`'s
  stopgap: `raise` an error whose **message** embeds code + message + requestId +
  status; model an operation's error set as a generated `enum` matched on the wire
  `code` (with a catch-all), upgrading to typed `aws.ServiceError` when I#12 lands.
  **v1 (stopgap form).**
- **Waiters** — poll-until-state; lower frequency, model-driven acceptors.
  **Deferred** to a fast-follow.

*Recommendation (Erg):* paginators + presign + per-op types + stopgap errors in
v1; waiters deferred.

### 8. Packaging & sequencing (forks Pkg, Seq)

**Package shape (botocore-vs-boto3 split):**

- **`aws-core`** — the hand-written runtime of §2 (signer, creds, endpoints,
  transport, retry, protocol codecs, pagination/waiter runtime, errors).
- **Per-service packages** — `aws-dynamodb`, `aws-sqs`, `aws-lambda`, … each
  generated and depending on `aws-core ^x`.
  Mirrors aws-sdk-rust's one-crate-per-service; the pkg manager downloads only what
  you import.
  Each stamps the model `uid`/`apiVersion` it was generated from (reproducible,
  diffable regen); model updates → CI regenerates → semver bump (minor = additive,
  ~all AWS changes).
- **`aws-lambda-runtime`** — the I#89(a) custom runtime; **separate**, needs no
  SigV4/SDK.

*Reject* a single mega-`aws` package (boto3-style): huge download/compile and one
version for ~400 independently-evolving services.

**First-wave services.**
Driven by §4 (JSON-only) and §3.1 (optionality):

- Under **O-a** (Option[T] lands): DynamoDB, SQS, Lambda (control plane + Invoke) —
  full read+write.
- Under **O-b** (no Option[T]): restrict the first wave to **read-oriented**
  operations where absent≈zero is safe — e.g. DynamoDB `GetItem`/`Query`/`Scan`,
  SQS `ReceiveMessage`, Lambda `Invoke`, STS/`GetCallerIdentity`-style reads (once
  XML lands), and later S3 `GetObject` (once XML lands).
  Writes/mutations wait for O-a or use O-c per operation.

**Sequencing (Seq) — recommended order:**

1. **`aws-lambda-runtime`** first — smallest, highest-value, needs only
   `http`+`json`+env creds; delivers the native-cold-start story immediately.
   (Independent of the SDK; a Lambda *handler* may later call services via the SDK,
   but the runtime loop does not need it.)
2. **`aws-core`** — signer + creds + transport + retry + JSON-family protocol
   codecs + pagination.
3. **First JSON-family service clients** (per §8 first-wave).
4. **`xml` module** (separate ticket) → then `rest-xml` + `query` protocols →
   **S3, STS** (unblocks AssumeRole/SSO creds), SNS, IAM, EC2.
5. **Fast-follows:** waiters, connection pooling, adaptive retries, SigV4A,
   token-direct codecs, the G3 dynamic escape-hatch module.

### 9. Binary size / DCE (why static beats boto3 here)

You pay for what you **reference**, not what you **import** — but only if the
generated code is shaped for it:

- **No eager global registry.**
  `Client` exposes operations as individually-reachable **methods**, never entries
  in a `name→fn` dispatch map; a table would pin every op's structs live and defeat
  `--gc-sections`.
  The G3 dynamic `call(name, …)` escape hatch, if built, lives in a **separate
  opt-in module** so importing *it* is what costs the full surface.
- **Chain of reachability:** calling `client.get_item(...)` keeps `get_item` →
  `GetItem{Input,Output}` → their derive codecs → that op's error enum; everything
  else is pruned.

This compounds with per-service packages (you never fetch/compile S3 to call
DynamoDB) and loops back to I#89: smaller binary = faster cold start.

## Rationale

- **Build-time codegen** preserves boto3's data-driven leverage (one engine over
  the models = all of AWS) while fitting Zap's AOT/no-reflection reality and typed
  pitch; it is the proven path for every compiled AWS SDK.
- **Models are the *what*, not the *how*** — so a bounded hand-written core
  (one signer, one cred chain, ~2 protocol codecs for v1) plus generated per-service
  code is the whole SDK, maintainable across ~300 services because the per-service
  part is mechanical.
- **Optionality is the real gate**, above XML: AWS is optional-heavy, and
  absent-vs-zero correctness needs `Option[T]`.
  Writing the doc to stand under both O-a and O-b keeps progress unblocked while the
  generics call is made.
- **JSON-first protocol coverage** lets v1 ship entirely on existing stdlib
  (`json`/`http`/`tls`/`crypto`); XML is a clean, separable second front.
- **Per-service packages + DCE** turn static typing into a binary-size and
  cold-start advantage over boto3, not just a typing nicety.
- **Lambda-runtime first** delivers I#89's headline value with the least
  dependency surface and is correctly independent of the SDK.

Rejected: runtime CPython botocore (G2 — kills cold starts, loses types);
mega-package (download/compile/versioning); SIMD/throughput optimizations and
SigV4A in v1 (scope).

## Compatibility

Purely additive and entirely outside the compiler/stdlib: `aws-*` are
official-tier packages installed via I#69, nothing AWS-specific enters the default
modules (D-0056).
The only asks *back* on landed components are additive (see Dependencies): a raw-
bytes HMAC in `crypto`, custom-field-codec + open-enum-fallback options in
`@derive(Json)`, and (for full coverage) a new `xml` module.
No grammar change.

## Implementation

Sequenced per §8.
The generator is a model→source transform (fork L): **Rust** for v1 (serde,
speed-to-first-service, lives in repo tooling) is the pragmatic pick; a **Zap**
generator (dogfooding `json`) is a later option — *flagged as a product call, not
decided here.*
Generated code is **checked in** (fork D) — reviewable, diffable, no build-time
generator dependency for users, and the natural fit for a pkg-manager that
distributes source; the generator runs in CI on model updates and opens bump PRs.
Model source (fork M) = **botocore `service-2.json`** for v1 (stable, one file per
service, exactly what boto3 uses); Smithy models are a future source.

Phase boundaries touched: **none in the compiler** (all in `zaplang-io` packages),
except the additive Dependencies asks, which are ordinary stdlib tickets.
Golden tests: SigV4 vectors (AWS published test suite + botocore oracle);
credential-chain resolution order; awsJson round-trip decode for a real DynamoDB/
SQS/Lambda operation; paginator over multi-page responses; retry/backoff on
injected throttles; presign URL vector.

## Dependencies (with how-blocking each is)

1. **`Option[T]` / user generics (D-0017)** — *load-bearing.*
   Governs optional-field correctness (§3.1).
   O-a needs it; without it the first wave is restricted to read-oriented ops (O-b)
   or uses presence flags (O-c).
   Blocks *faithful full coverage*, not the first read-oriented services.
2. **Raw-bytes HMAC in `crypto`** — *hard blocker for signing.*
   The SigV4 key chain feeds raw HMAC bytes into the next HMAC as key;
   `crypto.hmac_sha256` returns hex only.
   Need `crypto.hmac_sha256_raw(key, msg) -> bytes` (or a streaming HMAC handle).
   Small, additive `crypto` follow-up; **blocks `aws-core`**.
3. **`json` codec hooks** — *blocker for correct field decode.*
   Custom-field codecs for AWS timestamps (epoch/iso8601/rfc822) and base64 blobs
   (§3.3), plus an open-enum `Unknown(str)` fallback option (§3.2).
   Additive `@derive(Json)` options; **blocks correct models** but not the overall
   architecture.
4. **`xml` module** — *coverage blocker.*
   Gates `rest-xml` + `query` → **S3, STS, SNS, IAM, EC2** (§4).
   Its own stdlib/pkg ticket; blocks those services, not the JSON-family first wave.
   Note: STS being deferred also defers AssumeRole/SSO/web-identity credentials.
5. **`http` keep-alive / connection pooling** — *perf only, not a blocker.*
   v1 works one-request-per-connection; pooling is a fast-follow (§6).
6. **Typed exceptions (I#12)** — *ergonomics only.*
   v1 uses the string-message stopgap for errors (§7); upgrades API-compatibly when
   I#12 lands.

## Decisions (recommended — to be ratified in I#89 / follow-up tickets)

- **G — generation model:** build-time codegen → static Zap (G1).
  G3 dynamic client a later opt-in module; G2 (runtime botocore) only a test oracle.
- **M — model source:** botocore `service-2.json` for v1; Smithy later.
- **D — codegen output:** checked-in generated Zap source; generator runs in CI.
- **P — protocol coverage v1:** JSON-family only (awsJson + rest-json); rest-xml +
  query deferred to the `xml` work.
- **E — enums:** open enum with `Unknown(str)` fallback.
- **Ts — type conventions:** generator stamps timestamp/blob formats; `json` grows
  codec hooks.
- **A — auth:** SigV4 v1 (SigV4A deferred); credential chain = env + shared-file-
  static + IMDSv2 + container (STS/SSO/web-identity deferred).
- **X — transport/retry:** http+tls+netpoller + goroutines; standard-mode
  backoff+jitter v1; pooling + adaptive deferred.
- **Erg:** paginators + presign + per-op types + stopgap errors v1; waiters
  deferred.
- **Pkg:** `aws-core` + per-service packages + separate `aws-lambda-runtime`;
  reject mega-package.
- **Seq:** lambda-runtime → aws-core → JSON-family services → xml → S3/STS/etc.

## Open issues (explicit product calls — not decided here)

- **O (optionality):** O-a (schedule `Option[T]`, model optional properly,
  full first wave) **vs** O-b (ship read-oriented first wave now without it) **vs**
  O-c (presence flags).
  Preferred O-a; the orchestrator is making the generics-prioritization call and
  will relay it. This doc stands either way.
- **L (generator language):** Rust (recommended v1) vs Zap (dogfood).
- **S3 priority:** S3 is the most-wanted AWS service for many users but is
  `rest-xml` → it forces the `xml` module onto the critical path.
  Is S3-early worth prioritizing `xml` ahead of broader JSON-service coverage?
- **First-wave service list:** confirm DynamoDB / SQS / Lambda (+ which operations
  under O-b).
