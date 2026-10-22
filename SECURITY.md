# Security Policy

Zap is **pre-1.0 and pre-public**. There are no supported stable releases yet;
security fixes land on `main`. This policy will gain a supported-versions table
when the first tagged release ships.

## Reporting a vulnerability

**Please do not open a public issue, PR, or discussion for a suspected
vulnerability.** Report it privately using GitHub's private vulnerability
reporting:

1. Go to the repository's **Security** tab.
2. Choose **Report a vulnerability** (direct link:
   [Report a vulnerability](../../security/advisories/new)).
3. Describe the issue (see below). Only the maintainers can see the report.

If you cannot use GitHub's private reporting, open a minimal public issue that
says only that you have a security report and asks for a private channel —
**without any technical detail** — and a maintainer will follow up.

### What to include

- The affected component (compiler, runtime `zaprt.c`, a stdlib module, the
  `zap` CLI / package manager) and the commit or `zap version`.
- A minimal `.zap` program or input that reproduces it, and your platform.
- The impact as you understand it (memory corruption, code execution, data
  disclosure, denial of service, …) and any suggested fix.

### What to expect

This is a small project, so the commitments are modest but real:

- **Acknowledgement** of your report within a few days.
- An assessment and a plan, kept in the private advisory, and credit in the
  advisory/changelog if you want it.
- **Coordinated disclosure:** we ask that you give us a reasonable window to
  ship a fix before any public write-up, and we will tell you when it lands.

## What is in scope

Zap is a compiler plus a native runtime, so the interesting security surface is
different from a typical application. In scope:

- **Memory-safety bugs in generated code or the runtime** — the garbage
  collectors (precise Immix, conservative Boehm), the scheduler and netpoller,
  signal handling, and `zaprt.c` generally. A safe-looking `.zap` program that
  corrupts memory is a bug here, even if it needs a crafted input.
- **The compiler** accepting input that crashes it, hangs it, or makes it emit
  code that violates the language's guarantees.
- **The standard library**, in particular:
  - **`tls`** and the `https://` path of `http` — the client verifies the
    certificate chain *and* hostname by default (OpenSSL, system trust store);
    `tls.connect_insecure` is a deliberate, loud opt-out and is not itself a
    vulnerability. A server-side TLS surface is **deferred** and not yet public
    API; the internal `tls._test_server` is a test scaffold, not a supported
    server.
  - **`crypto`** — hashing, HMAC, and secure random. The reference
    implementations of SHA-1 and MD5 are provided for interoperability and are
    **not** recommended for security-sensitive use; that is documentation, not a
    vulnerability. Flaws in the CSPRNG, HMAC, or digest correctness are in
    scope. Constant-time comparison and a broader crypto surface are future
    work.
  - `zlib`/`gzip`, `yaml`, `json`, `net`/`http`, and other parsers of untrusted
    input.
- **The package manager** (`zap add`/`remove`, `zap.toml`, the registry once it
  exists) — dependency confusion, integrity/verification bypass, path traversal.

Out of scope: vulnerabilities in third-party libraries Zap links (OpenSSL,
libgc, PCRE2, zlib, …) themselves — report those upstream, though please tell us
if Zap's *use* of them is unsafe; issues that require an attacker who already
controls the build machine or the compiler binary; and findings against
unreleased design documents.

## Dual license

Zap is available under [Apache-2.0](LICENSE-APACHE) **or** [MIT](LICENSE-MIT),
at your option. Both licenses disclaim warranty; reporting a vulnerability does
not create any additional obligation, and contributions of a fix are accepted
under the same dual license as described in [CONTRIBUTING.md](CONTRIBUTING.md).
