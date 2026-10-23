//! Comment preservation: the formatted output must retain exactly the same
//! comment lexemes (normalized) as the input — none dropped, none invented.
use std::fs;
use std::path::PathBuf;

fn comment_texts(src: &str) -> Vec<String> {
    let (_t, _e, comments) = zapc_syntax::lexer::tokenize_with_trivia(src);
    comments
        .into_iter()
        .map(|c| {
            // normalize the same way the formatter does (space after #)
            let hashes = c.text.chars().take_while(|&ch| ch == '#').count();
            let (h, rest) = c.text.split_at(hashes);
            match rest.chars().next() {
                Some(ch) if ch != ' ' && ch != '!' && ch != ':' => format!("{h} {rest}"),
                _ => c.text,
            }
        })
        .collect()
}

#[test]
fn no_comment_is_lost() {
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
        let before = comment_texts(&src);
        let out = zapc_fmt::format_source(&src).expect("format");
        let after = comment_texts(&out);
        if before != after {
            reports.push(format!(
                "{name}: comments differ ({} -> {})",
                before.len(),
                after.len()
            ));
        }
    }
    assert!(reports.is_empty(), "comment loss:\n{}", reports.join("\n"));
}
