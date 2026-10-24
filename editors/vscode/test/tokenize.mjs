// A lightweight, dependency-light check that syntaxes/zap.tmLanguage.json
// actually tokenizes Zap source into the scopes the editor relies on. It loads
// the real grammar through the same engine VS Code uses (vscode-textmate +
// vscode-oniguruma), tokenizes test/sample.zap, and asserts that a handful of
// representative tokens land in the expected scope families. It is a smoke test
// for the grammar, not a full golden of every token.

import { readFileSync } from "fs";
import { fileURLToPath } from "url";
import { dirname, join } from "path";
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const oniguruma = require("vscode-oniguruma");
const vsctm = require("vscode-textmate");

const here = dirname(fileURLToPath(import.meta.url));
const grammarPath = join(here, "..", "syntaxes", "zap.tmLanguage.json");
const samplePath = join(here, "sample.zap");

async function main() {
  const wasmPath = require.resolve("vscode-oniguruma/release/onig.wasm");
  const wasmBin = readFileSync(wasmPath).buffer;
  const onigLib = oniguruma.loadWASM(wasmBin).then(() => ({
    createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
    createOnigString: (s) => new oniguruma.OnigString(s),
  }));

  const registry = new vsctm.Registry({
    onigLib,
    loadGrammar: async (scopeName) => {
      if (scopeName === "source.zap") {
        const data = readFileSync(grammarPath, "utf8");
        return vsctm.parseRawGrammar(data, grammarPath);
      }
      return null;
    },
  });

  const grammar = await registry.loadGrammar("source.zap");
  if (!grammar) {
    throw new Error("failed to load source.zap grammar");
  }

  // Tokenize the sample line by line, carrying the rule stack across lines so
  // multi-line constructs (triple-quoted docstrings) tokenize correctly.
  const text = readFileSync(samplePath, "utf8");
  const lines = text.split(/\r?\n/);
  const tokens = []; // { text, scopes }
  let ruleStack = vsctm.INITIAL;
  for (const line of lines) {
    const result = grammar.tokenizeLine(line, ruleStack);
    for (const t of result.tokens) {
      tokens.push({
        text: line.substring(t.startIndex, t.endIndex),
        scopes: t.scopes,
      });
    }
    ruleStack = result.ruleStack;
  }

  // Each check: some token whose trimmed text matches `text` must carry a scope
  // matching `scope`.
  const checks = [
    { what: "def keyword", text: /^def$/, scope: /\b(keyword|storage\.type\.function)/ },
    { what: "struct keyword", text: /^struct$/, scope: /\b(keyword|storage\.type)/ },
    { what: "return keyword", text: /^return$/, scope: /keyword\.control/ },
    { what: "boolean/none literal", text: /^(True|None)$/, scope: /constant\.language/ },
    { what: "builtin type", text: /^int64$/, scope: /support\.type/ },
    { what: "storage modifier", text: /^(const|auto)$/, scope: /storage\.modifier/ },
    { what: "word operator", text: /^and$/, scope: /keyword\.operator\.word/ },
    { what: "integer literal", text: /^42$/, scope: /constant\.numeric/ },
    { what: "float literal", text: /^3\.14$/, scope: /constant\.numeric\.float/ },
    { what: "def name", text: /^fib$/, scope: /entity\.name\.function/ },
    { what: "type name", text: /^Point$/, scope: /entity\.name\.type/ },
    { what: "self", text: /^self$/, scope: /variable\.language/ },
  ];

  // Scope-family-only checks (the token text varies): at least one token must
  // carry the scope.
  const scopeChecks = [
    { what: "line comment", scope: /comment\.line\.number-sign/ },
    { what: "quoted string", scope: /string\.quoted/ },
    { what: "docstring", scope: /string\.quoted\.docstring/ },
    { what: "decorator", scope: /entity\.name\.function\.decorator/ },
  ];

  const failures = [];
  for (const c of checks) {
    const hit = tokens.some(
      (t) => c.text.test(t.text.trim()) && t.scopes.some((s) => c.scope.test(s)),
    );
    if (!hit) {
      failures.push(`missing ${c.what}: no token matching ${c.text} with scope ${c.scope}`);
    }
  }
  for (const c of scopeChecks) {
    const hit = tokens.some((t) => t.scopes.some((s) => c.scope.test(s)));
    if (!hit) {
      failures.push(`missing ${c.what}: no token with scope ${c.scope}`);
    }
  }

  if (failures.length > 0) {
    console.error("TextMate grammar tokenizer test FAILED:");
    for (const f of failures) {
      console.error("  - " + f);
    }
    process.exit(1);
  }

  console.log(`TextMate grammar tokenizer test passed (${tokens.length} tokens, ${checks.length + scopeChecks.length} checks).`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
