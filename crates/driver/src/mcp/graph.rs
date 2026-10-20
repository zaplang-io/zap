//! Code graph / packed memory (D-0055 surface 3).
//!
//! Builds a compact semantic index of a Zap codebase — every top-level symbol
//! (functions, structs, enums, interfaces, annotations, decorators) with its
//! rendered signature, file, and line, plus the members of containers (struct
//! fields/methods, enum variants, interface methods). Agents query this index
//! for the shape of a project at a fraction of the token cost of reading raw
//! source.
//!
//! It is built from the front-end only (parse; no type-check needed), so it is
//! host-safe and works on code that does not yet fully type-check.

use std::path::{Path, PathBuf};

use zapc_syntax::ast::{self, EnumDef, Function, InterfaceDef, Item, StructDef, Type};

use super::json::Json;
use super::span_to_line_col;

/// One indexed symbol.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// `function`, `struct`, `enum`, `interface`, `annotation`, `decorator`,
    /// and for members `field`, `method`, `variant`.
    pub kind: &'static str,
    pub name: String,
    /// A one-line, source-like rendering of the declaration.
    pub signature: String,
    pub file: String,
    pub line: u32,
    /// Summary line of a docstring, when present (functions/methods).
    pub doc: Option<String>,
    /// Members (struct fields/methods, enum variants, interface methods).
    pub children: Vec<Symbol>,
}

impl Symbol {
    fn to_json(&self) -> Json {
        let mut pairs = vec![
            ("kind", Json::str(self.kind)),
            ("name", Json::str(self.name.clone())),
            ("signature", Json::str(self.signature.clone())),
            ("file", Json::str(self.file.clone())),
            ("line", Json::int(self.line as i64)),
        ];
        if let Some(doc) = &self.doc {
            pairs.push(("doc", Json::str(doc.clone())));
        }
        if !self.children.is_empty() {
            pairs.push((
                "children",
                Json::Arr(self.children.iter().map(Symbol::to_json).collect()),
            ));
        }
        Json::obj(pairs)
    }

    /// A single packed text line (indented for members), token-efficient for an
    /// agent to read in bulk.
    fn packed(&self, out: &mut String, indent: usize) {
        for _ in 0..indent {
            out.push_str("  ");
        }
        out.push_str(&self.signature);
        if indent == 0 {
            out.push_str(&format!("  [{}:{}]", self.file, self.line));
        }
        out.push('\n');
        for c in &self.children {
            c.packed(out, indent + 1);
        }
    }
}

/// The whole-codebase index.
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub files: Vec<String>,
    pub symbols: Vec<Symbol>,
}

impl Index {
    /// Build an index of every `.zap` file at or below `root`.
    pub fn build(root: &Path) -> Index {
        let mut files = Vec::new();
        collect_zap(root, &mut files);
        files.sort();
        let mut index = Index::default();
        for path in files {
            let display = path.display().to_string();
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            let (module, _perrs, _lerrs) = zapc_syntax::parser::parse_str(&src);
            // Over-tolerant on purpose: even a file with parse errors yields the
            // items that did parse, which is still useful context for an agent.
            index_module(&module, &display, &src, &mut index.symbols);
            index.files.push(display);
        }
        index
    }

    /// Filter to symbols matching an optional name substring (case-insensitive)
    /// and/or an exact kind. A match on a top-level symbol returns the whole
    /// symbol (with its members); a match only on a member returns the member's
    /// container so the context is preserved.
    pub fn query<'a>(&'a self, name: Option<&str>, kind: Option<&str>) -> Vec<&'a Symbol> {
        let name_lc = name.map(|n| n.to_lowercase());
        self.symbols
            .iter()
            .filter(|s| {
                // `map_or(true, …)` rather than `is_none_or` to stay within the
                // crate's MSRV (`rust-version = 1.74`; `is_none_or` is 1.82+).
                let kind_ok = kind.map_or(true, |k| {
                    s.kind == k || s.children.iter().any(|c| c.kind == k)
                });
                let name_ok = name_lc.as_ref().map_or(true, |n| {
                    s.name.to_lowercase().contains(n)
                        || s.children.iter().any(|c| c.name.to_lowercase().contains(n))
                });
                kind_ok && name_ok
            })
            .collect()
    }

    /// The JSON payload for a `zap_code_graph` response over `matches`.
    pub fn to_json(&self, matches: &[&Symbol]) -> Json {
        let mut packed = String::new();
        for s in matches {
            s.packed(&mut packed, 0);
        }
        Json::obj(vec![
            ("files", Json::int(self.files.len() as i64)),
            ("symbols", Json::int(self.symbols.len() as i64)),
            ("matched", Json::int(matches.len() as i64)),
            (
                "results",
                Json::Arr(matches.iter().map(|s| s.to_json()).collect()),
            ),
            ("packed", Json::str(packed)),
        ])
    }
}

fn index_module(module: &ast::Module, file: &str, src: &str, out: &mut Vec<Symbol>) {
    for item in &module.items {
        match item {
            Item::Function(f) => out.push(function_symbol(f, file, src)),
            Item::Struct(s) => out.push(struct_symbol(s, file, src)),
            Item::Enum(e) => out.push(enum_symbol(e, file, src)),
            Item::Interface(i) => out.push(interface_symbol(i, file, src)),
            Item::Annotation(a) => out.push(Symbol {
                kind: "annotation",
                name: a.name.clone(),
                signature: format!("annotation {}", a.name),
                file: file.to_string(),
                line: span_to_line_col(src, a.span.start).0,
                doc: None,
                children: a
                    .fields
                    .iter()
                    .map(|fld| Symbol {
                        kind: "field",
                        name: fld.name.clone(),
                        signature: format!("{} {}", render_type(&fld.ty), fld.name),
                        file: file.to_string(),
                        line: span_to_line_col(src, fld.span.start).0,
                        doc: None,
                        children: Vec::new(),
                    })
                    .collect(),
            }),
            Item::Decorator(d) => out.push(Symbol {
                kind: "decorator",
                name: d.name.clone(),
                signature: format!("decorator {}({})", d.name, render_params(&d.params)),
                file: file.to_string(),
                line: span_to_line_col(src, d.span.start).0,
                doc: None,
                children: Vec::new(),
            }),
            // Bare top-level statements and imports are not symbols.
            Item::Import(_) | Item::Stmt(_) => {}
        }
    }
}

fn function_symbol(f: &Function, file: &str, src: &str) -> Symbol {
    Symbol {
        kind: "function",
        name: f.name.clone(),
        signature: render_fn_sig("fn", f),
        file: file.to_string(),
        line: span_to_line_col(src, f.span.start).0,
        doc: ast::docstring(f).map(doc_summary),
        children: Vec::new(),
    }
}

fn struct_symbol(s: &StructDef, file: &str, src: &str) -> Symbol {
    let kw = if s.is_value { "value struct" } else { "struct" };
    let mut children: Vec<Symbol> = s
        .fields
        .iter()
        .map(|fld| Symbol {
            kind: "field",
            name: fld.name.clone(),
            signature: format!("{} {}", render_type(&fld.ty), fld.name),
            file: file.to_string(),
            line: span_to_line_col(src, fld.span.start).0,
            doc: None,
            children: Vec::new(),
        })
        .collect();
    children.extend(s.methods.iter().map(|m| Symbol {
        kind: "method",
        name: m.name.clone(),
        signature: render_fn_sig("method", m),
        file: file.to_string(),
        line: span_to_line_col(src, m.span.start).0,
        doc: ast::docstring(m).map(doc_summary),
        children: Vec::new(),
    }));
    Symbol {
        kind: "struct",
        name: s.name.clone(),
        signature: format!("{kw} {}{}", s.name, render_generics(&s.generics)),
        file: file.to_string(),
        line: span_to_line_col(src, s.span.start).0,
        doc: None,
        children,
    }
}

fn enum_symbol(e: &EnumDef, file: &str, src: &str) -> Symbol {
    let mut children: Vec<Symbol> = e
        .variants
        .iter()
        .map(|v| {
            let sig = if v.fields.is_empty() {
                v.name.clone()
            } else {
                let fields = v
                    .fields
                    .iter()
                    .map(|vf| format!("{} {}", render_type(&vf.ty), vf.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({fields})", v.name)
            };
            Symbol {
                kind: "variant",
                name: v.name.clone(),
                signature: sig,
                file: file.to_string(),
                line: span_to_line_col(src, v.span.start).0,
                doc: None,
                children: Vec::new(),
            }
        })
        .collect();
    children.extend(e.methods.iter().map(|m| Symbol {
        kind: "method",
        name: m.name.clone(),
        signature: render_fn_sig("method", m),
        file: file.to_string(),
        line: span_to_line_col(src, m.span.start).0,
        doc: ast::docstring(m).map(doc_summary),
        children: Vec::new(),
    }));
    Symbol {
        kind: "enum",
        name: e.name.clone(),
        signature: format!("enum {}{}", e.name, render_generics(&e.generics)),
        file: file.to_string(),
        line: span_to_line_col(src, e.span.start).0,
        doc: None,
        children,
    }
}

fn interface_symbol(i: &InterfaceDef, file: &str, src: &str) -> Symbol {
    let children = i
        .methods
        .iter()
        .map(|m| {
            let ret = m
                .ret
                .as_ref()
                .map(|t| format!(" -> {}", render_type(t)))
                .unwrap_or_default();
            Symbol {
                kind: "method",
                name: m.name.clone(),
                signature: format!("{}({}){ret}", m.name, render_params(&m.params)),
                file: file.to_string(),
                line: span_to_line_col(src, m.span.start).0,
                doc: None,
                children: Vec::new(),
            }
        })
        .collect();
    Symbol {
        kind: "interface",
        name: i.name.clone(),
        signature: format!("interface {}{}", i.name, render_generics(&i.generics)),
        file: file.to_string(),
        line: span_to_line_col(src, i.span.start).0,
        doc: None,
        children,
    }
}

/// Render a function/method signature: `<kw> name[generics](params) -> ret`.
fn render_fn_sig(kw: &str, f: &Function) -> String {
    let ret = f
        .ret
        .as_ref()
        .map(|t| format!(" -> {}", render_type(t)))
        .unwrap_or_default();
    format!(
        "{kw} {}{}({}){ret}",
        f.name,
        render_generics(&f.generics),
        render_params(&f.params)
    )
}

fn render_generics(generics: &[ast::TypeParam]) -> String {
    if generics.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = generics.iter().map(|p| p.name.as_str()).collect();
        format!("[{}]", names.join(", "))
    }
}

fn render_params(params: &[ast::Param]) -> String {
    params
        .iter()
        .map(|p| {
            let ty = p.ty.as_ref().map(render_type).unwrap_or_default();
            let sep = if p.variadic { "... " } else { " " };
            let base = if ty.is_empty() {
                // An untyped `self` receiver.
                p.name.clone()
            } else {
                format!("{ty}{sep}{}", p.name)
            };
            match &p.default {
                Some(d) => format!("{base} = {}", ast::render_expr(d)),
                None => base,
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a type back to its source spelling.
pub fn render_type(t: &Type) -> String {
    match t {
        Type::Named { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner = args.iter().map(render_type).collect::<Vec<_>>().join(", ");
                format!("{name}[{inner}]")
            }
        }
        Type::Tuple { elems, .. } => {
            let inner = elems.iter().map(render_type).collect::<Vec<_>>().join(", ");
            format!("({inner})")
        }
        Type::Func { params, ret, .. } => {
            let ps = params
                .iter()
                .map(render_type)
                .collect::<Vec<_>>()
                .join(", ");
            match ret {
                Some(r) => format!("fn({ps}) -> {}", render_type(r)),
                None => format!("fn({ps})"),
            }
        }
    }
}

/// The first non-empty line of a docstring, trimmed.
fn doc_summary(doc: &str) -> String {
    doc.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Collect every `.zap` file at or below `dir`, skipping build/VCS dirs. A
/// single file path is accepted directly.
fn collect_zap(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_file() {
        if dir.extension().and_then(|e| e.to_str()) == Some("zap") {
            out.push(dir.to_path_buf());
        }
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if matches!(
                name.as_ref(),
                "target" | ".git" | ".claude" | "node_modules"
            ) || name.starts_with('.')
            {
                continue;
            }
            collect_zap(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("zap") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, src: &str) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(p).unwrap();
        f.write_all(src.as_bytes()).unwrap();
    }

    fn tmpdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "zap-graph-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn indexes_functions_structs_enums() {
        let dir = tmpdir();
        write(
            &dir,
            "a.zap",
            "def fib(int64 n) -> int64:\n    return n\n\
             struct Point:\n    int64 x\n    int64 y\n\
             enum Color:\n    RED\n    GREEN\n",
        );
        let index = Index::build(&dir);
        let names: Vec<_> = index.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"fib"));
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"Color"));

        let fib = index.symbols.iter().find(|s| s.name == "fib").unwrap();
        assert_eq!(fib.kind, "function");
        assert!(fib.signature.contains("fib(int64 n) -> int64"));

        let point = index.symbols.iter().find(|s| s.name == "Point").unwrap();
        assert_eq!(point.children.len(), 2);
        assert!(point.children.iter().any(|c| c.name == "x"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_by_name_and_kind() {
        let dir = tmpdir();
        write(
            &dir,
            "a.zap",
            "def alpha():\n    pass\ndef beta():\n    pass\nstruct Gamma:\n    int64 z\n",
        );
        let index = Index::build(&dir);

        let by_name = index.query(Some("alph"), None);
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].name, "alpha");

        let structs = index.query(None, Some("struct"));
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Gamma");

        let json = index.to_json(&by_name).to_string();
        assert!(json.contains("\"matched\":1"));
        assert!(json.contains("alpha"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_type_handles_generics_and_tuples() {
        let (m, _p, _l) = zapc_syntax::parser::parse_str(
            "def f(dict[str, int64] d, (int64, str) t):\n    pass\n",
        );
        if let Item::Function(f) = &m.items[0] {
            let sig = render_fn_sig("fn", f);
            assert!(sig.contains("dict[str, int64] d"), "sig was {sig}");
            assert!(sig.contains("(int64, str) t"), "sig was {sig}");
        } else {
            panic!("expected a function");
        }
    }
}
