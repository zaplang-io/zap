//! The strongest corpus guarantee: formatting preserves meaning. We compare the
//! parsed AST before and after formatting, with spans masked out (the formatter
//! changes byte offsets but must not change structure).
use std::fs;
use std::path::PathBuf;

fn mask_spans(debug: &str) -> String {
    // AST nodes derive Debug; spans appear as `Span { start: N, end: M }`.
    // Char-boundary-safe: a `.zap` source (hence its debug dump) may contain
    // multibyte UTF-8, so scan by substring rather than by byte index.
    let mut out = String::with_capacity(debug.len());
    let mut rest = debug;
    while let Some(pos) = rest.find("Span { start: ") {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        match rest.find('}') {
            // '}' is ASCII, so `close + 1` is a char boundary.
            Some(close) => {
                out.push_str("Span");
                rest = &rest[close + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn formatting_preserves_ast() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    let mut paths: Vec<PathBuf> = fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "zap").unwrap_or(false))
        .collect();
    paths.sort();
    let mut reports = Vec::new();
    for path in paths {
        let src = fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let (a, _, _, _) = zapc_syntax::parser::parse_str_with_trivia(&src);
        let out = zapc_fmt::format_source(&src).expect("format");
        let (b, _, _, _) = zapc_syntax::parser::parse_str_with_trivia(&out);
        let da = mask_spans(&format!("{a:?}"));
        let db = mask_spans(&format!("{b:?}"));
        if da != db {
            // find first differing line for a compact report
            let first_diff = da
                .lines()
                .zip(db.lines())
                .find(|(x, y)| x != y)
                .map(|(x, y)| format!("    - {}\n    + {}", x.trim(), y.trim()))
                .unwrap_or_default();
            reports.push(format!("{name}: AST changed\n{first_diff}"));
        }
    }
    assert!(
        reports.is_empty(),
        "{} semantic mismatches:\n{}",
        reports.len(),
        reports.join("\n")
    );
}
