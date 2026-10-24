# Zap for VS Code

The official VS Code extension for the [Zap](https://github.com/parveshgarg/zap)
programming language. It provides:

- **Syntax highlighting** via a TextMate grammar grounded in Zap's real grammar
  (`docs/GRAMMAR.ebnf`) and keyword set.
- **Language intelligence** — live diagnostics (with stable `ZAPnnnn` codes),
  completion, hover, go-to-definition and document symbols — by launching the
  `zap lsp` language server that ships with the Zap toolchain.

This is a **thin client**: all analysis happens in `zap lsp`. The extension only
resolves the `zap` binary, starts the LSP client, and wires the `zap` language.

## Requirements

A `zap` executable must be available — either on your `PATH`, or pointed at by
the `zap.lsp.serverPath` setting. Build it from the repo root with:

```sh
cargo build --release --bin zap
# the binary is then at target/release/zap
```

## Settings

| Setting | Default | Description |
| --- | --- | --- |
| `zap.lsp.serverPath` | `""` | Path to the `zap` executable. Empty → use `zap` on `PATH`. Supports `~` and workspace-relative paths. |
| `zap.trace.server` | `off` | LSP trace verbosity (`off` / `messages` / `verbose`). |

## Commands

- **Zap: Restart Language Server** (`zap.restartServer`) — stops and relaunches
  the server (e.g. after rebuilding `zap`).

## Build / run / debug

From this directory (`editors/vscode/`):

```sh
npm ci              # install dependencies (uses the committed package-lock.json)
npm run compile     # type-check + emit out/extension.js (tsc)
npm test            # tokenize test/sample.zap through the TextMate grammar
```

To run it interactively, open `editors/vscode/` in VS Code and press **F5**
("Run Extension"). This launches an Extension Development Host; open any `.zap`
file in it to activate the extension. Set `zap.lsp.serverPath` in the dev host
to your freshly built `target/debug/zap` (or `target/release/zap`), or make sure
`zap` is on `PATH`. Use **Zap: Restart Language Server** after rebuilding the
server.

## Packaging a `.vsix`

```sh
npm run package     # runs `vsce package`, producing zap-lang-<version>.vsix
```

Install the resulting `.vsix` with
`code --install-extension zap-lang-<version>.vsix`, or via the Extensions view's
"Install from VSIX…" command. Publishing to the Marketplace is an
org/account action and is intentionally **not** part of this repo's automation.

## Scope notes

- The TextMate grammar highlights keywords, built-in types, decorators,
  function/type definition names, strings (including triple-quoted docstrings
  and `f`-string interpolation), numbers, constants and operators. Regex
  literals (`m/…/`, `s/…/…/`) are highlighted only in the common `/`-delimited
  form; Zap's arbitrary-delimiter regex syntax is not covered by the static
  grammar (semantic highlighting is the server's job).
- Deep/semantic highlighting and all navigation come from `zap lsp`, not from
  the grammar.
