//! Test results: the model, the stdout-sentinel parser, and the human / `--json`
//! renderers (D-0054). All of this is pure and host-testable — it never touches
//! LLVM — so the runner's output is covered by `cargo test` without a backend.

use super::harness::MARKER;

/// The outcome of a single test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

impl Status {
    fn parse(s: &str) -> Option<Status> {
        match s {
            "PASS" => Some(Status::Pass),
            "FAIL" => Some(Status::Fail),
            "SKIP" => Some(Status::Skip),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::Skip => "skip",
        }
    }
}

/// One test's result, with any failure/skip detail and captured stdout.
#[derive(Clone, Debug)]
pub struct TestResult {
    pub file: String,
    pub name: String,
    pub status: Status,
    /// Failure diagnostic (power-assert text) or skip reason; empty otherwise.
    pub message: String,
    /// The test's own stdout, retained so a failure can show it.
    pub output: String,
}

/// Parse one test binary's stdout into results, tagging each with `file`.
///
/// The parser is forgiving: a binary that crashes partway leaves its in-progress
/// test without a `RESULT` line, which is reported as a failure by the caller
/// (via [`crate::testing`]'s exit-status handling), not here.
pub fn parse_output(file: &str, stdout: &str) -> Vec<TestResult> {
    let mut results = Vec::new();
    let mut cur_name: Option<String> = None;
    let mut output = String::new();
    let mut message = String::new();
    let mut in_msg = false;

    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix(MARKER) {
            let rest = rest.trim_start();
            let mut parts = rest.splitn(3, ' ');
            match parts.next() {
                Some("BEGIN") => {
                    cur_name = parts.next().map(|s| s.to_string());
                    output.clear();
                    message.clear();
                    in_msg = false;
                }
                Some("MSGSTART") => in_msg = true,
                Some("MSGEND") => in_msg = false,
                Some("RESULT") => {
                    let name = parts.next().unwrap_or("").to_string();
                    let status = parts.next().and_then(Status::parse).unwrap_or(Status::Fail);
                    results.push(TestResult {
                        file: file.to_string(),
                        name: if name.is_empty() {
                            cur_name.clone().unwrap_or_default()
                        } else {
                            name
                        },
                        status,
                        message: message.trim_end().to_string(),
                        output: output.clone(),
                    });
                    cur_name = None;
                    output.clear();
                    message.clear();
                    in_msg = false;
                }
                _ => {}
            }
            continue;
        }
        // Ordinary output line.
        if cur_name.is_none() {
            continue; // stray output outside a test (e.g. before_all) — ignore
        }
        if in_msg {
            message.push_str(line);
            message.push('\n');
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }

    results
}

/// Tallies across all results.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl Summary {
    pub fn of(results: &[TestResult]) -> Summary {
        let mut s = Summary {
            total: results.len(),
            ..Default::default()
        };
        for r in results {
            match r.status {
                Status::Pass => s.passed += 1,
                Status::Fail => s.failed += 1,
                Status::Skip => s.skipped += 1,
            }
        }
        s
    }

    pub fn ok(self) -> bool {
        self.failed == 0
    }
}

/// Render the human-readable report. Returns `(text, ok)`.
pub fn format_human(results: &[TestResult]) -> (String, bool) {
    let mut out = String::new();
    let mut last_file: Option<&str> = None;

    for r in results {
        if last_file != Some(r.file.as_str()) {
            out.push_str(&format!("\n{}\n", r.file));
            last_file = Some(r.file.as_str());
        }
        let tag = match r.status {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
        };
        out.push_str(&format!("  [{tag}] {}\n", r.name));
        if r.status == Status::Skip && !r.message.is_empty() {
            out.push_str(&format!("        reason: {}\n", r.message));
        }
        if r.status == Status::Fail {
            if !r.output.trim().is_empty() {
                for l in r.output.trim_end().lines() {
                    out.push_str(&format!("        | {l}\n"));
                }
            }
            for l in r.message.lines() {
                out.push_str(&format!("        {l}\n"));
            }
        }
    }

    let s = Summary::of(results);
    out.push_str(&format!(
        "\n{} passed, {} failed, {} skipped ({} total)\n",
        s.passed, s.failed, s.skipped, s.total
    ));
    (out, s.ok())
}

/// Render the structured `--json` report (hand-built so the driver stays
/// dependency-free). Consumed by `zap mcp` (#72).
pub fn format_json(results: &[TestResult]) -> String {
    let mut out = String::from("{\n  \"tests\": [");
    for (i, r) in results.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("\n    {");
        out.push_str(&format!("\"file\": {}, ", json_str(&r.file)));
        out.push_str(&format!("\"name\": {}, ", json_str(&r.name)));
        out.push_str(&format!("\"status\": \"{}\", ", r.status.as_str()));
        out.push_str(&format!("\"message\": {}, ", json_str(&r.message)));
        out.push_str(&format!("\"output\": {}", json_str(&r.output)));
        out.push('}');
    }
    if !results.is_empty() {
        out.push_str("\n  ");
    }
    out.push_str("],\n");
    let s = Summary::of(results);
    out.push_str("  \"summary\": {");
    out.push_str(&format!("\"total\": {}, ", s.total));
    out.push_str(&format!("\"passed\": {}, ", s.passed));
    out.push_str(&format!("\"failed\": {}, ", s.failed));
    out.push_str(&format!("\"skipped\": {}, ", s.skipped));
    out.push_str(&format!("\"ok\": {}", s.ok()));
    out.push_str("}\n}\n");
    out
}

/// Minimal JSON string escaping.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: &str = MARKER;

    #[test]
    fn parses_pass_and_fail() {
        let stdout = format!(
            "{M} BEGIN test_a\n{M} RESULT test_a PASS\n\
             {M} BEGIN test_b\nsome output\n{M} MSGSTART\nassertion failed\n  left:  1\n{M} MSGEND\n{M} RESULT test_b FAIL\n"
        );
        let r = parse_output("math_test.zap", &stdout);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].name, "test_a");
        assert_eq!(r[0].status, Status::Pass);
        assert_eq!(r[1].name, "test_b");
        assert_eq!(r[1].status, Status::Fail);
        assert!(r[1].message.contains("assertion failed"));
        assert!(r[1].message.contains("left:  1"));
        assert!(r[1].output.contains("some output"));
    }

    #[test]
    fn parses_skip_with_reason() {
        let stdout = format!(
            "{M} BEGIN test_c\n{M} MSGSTART\nCI only\n{M} MSGEND\n{M} RESULT test_c SKIP\n"
        );
        let r = parse_output("f.zap", &stdout);
        assert_eq!(r[0].status, Status::Skip);
        assert_eq!(r[0].message, "CI only");
    }

    #[test]
    fn summary_counts() {
        let stdout = format!(
            "{M} BEGIN a\n{M} RESULT a PASS\n{M} BEGIN b\n{M} RESULT b FAIL\n{M} BEGIN c\n{M} RESULT c SKIP\n"
        );
        let s = Summary::of(&parse_output("f.zap", &stdout));
        assert_eq!(
            s,
            Summary {
                total: 3,
                passed: 1,
                failed: 1,
                skipped: 1
            }
        );
        assert!(!s.ok());
    }

    #[test]
    fn human_output_has_summary_and_markers() {
        let stdout = format!("{M} BEGIN a\n{M} RESULT a PASS\n");
        let (text, ok) = format_human(&parse_output("f.zap", &stdout));
        assert!(ok);
        assert!(text.contains("[PASS] a"));
        assert!(text.contains("1 passed, 0 failed, 0 skipped (1 total)"));
    }

    #[test]
    fn json_output_is_well_formed_and_escapes() {
        let stdout = format!(
            "{M} BEGIN a\n{M} MSGSTART\nline\"with\\quote\n{M} MSGEND\n{M} RESULT a FAIL\n"
        );
        let json = format_json(&parse_output("f.zap", &stdout));
        assert!(json.contains("\"status\": \"fail\""));
        assert!(json.contains("\\\"with\\\\quote"));
        assert!(json.contains("\"ok\": false"));
        assert!(json.contains("\"total\": 1"));
    }
}
