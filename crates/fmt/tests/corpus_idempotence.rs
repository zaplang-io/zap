//! Property tests over the real `.zap` corpus: formatting must (1) succeed,
//! (2) produce output that still parses with no errors, and (3) be idempotent.
use std::fs;
use std::path::PathBuf;

fn corpus() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    let mut v: Vec<PathBuf> = fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "zap").unwrap_or(false))
        .collect();
    v.sort();
    v
}

#[test]
fn corpus_formats_parses_and_is_idempotent() {
    let mut fail = 0usize;
    let mut reports = Vec::new();
    for path in corpus() {
        let src = fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let once = match zapc_fmt::format_source(&src) {
            Ok(s) => s,
            Err(e) => {
                fail += 1;
                reports.push(format!("{name}: format error: {e}"));
                continue;
            }
        };
        // Formatted output must parse with no errors.
        let (_m, perrs, lerrs, _c) = zapc_syntax::parser::parse_str_with_trivia(&once);
        if !perrs.is_empty() || !lerrs.is_empty() {
            fail += 1;
            reports.push(format!(
                "{name}: formatted output does not parse: {perrs:?} {lerrs:?}"
            ));
            continue;
        }
        // Idempotence.
        match zapc_fmt::format_source(&once) {
            Ok(twice) if twice == once => {}
            Ok(_twice) => {
                fail += 1;
                reports.push(format!("{name}: NOT idempotent"));
            }
            Err(e) => {
                fail += 1;
                reports.push(format!("{name}: second format errored: {e}"));
            }
        }
    }
    assert_eq!(fail, 0, "{} corpus failures:\n{}", fail, reports.join("\n"));
}
