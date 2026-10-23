//! Repo/org coding conventions (D-0055 surface 5, issue #108).
//!
//! Lets an **organization check a conventions file into its repository** that
//! `zap mcp` discovers and serves to coding agents, so an LLM writes code in
//! *this repo's* house style — naming, error-handling patterns, preferred
//! stdlib modules, layering rules, testing requirements, do's and don'ts.
//!
//! This is distinct from [`super::reference`], which teaches *Zap the
//! language* from documents embedded in the toolchain binary. Conventions are
//! **repo-specific**: discovered at runtime under the server's codebase root,
//! never embedded. When no file is present the surface degrades gracefully and
//! the rest of the server is unaffected.
//!
//! It is exposed two ways, mirroring the reference surface: as an MCP
//! **resource** ([`URI`]) for resource-aware clients, and through the
//! `conventions` **tool** for tool-only clients. The server also advertises it
//! in its `initialize` instructions so agents consult it *before* writing code.
//!
//! **MVP scope (issue #108).** A single repo-root file; free-form markdown (no
//! schema); a soft size cap with truncation-on-notice because the content is
//! injected into agent context. Deferred: nested/hierarchical per-directory
//! merge, a user/global file merged under the repo one, section-aware parsing,
//! ingesting an existing `AGENTS.md`/`CLAUDE.md`, and linting the file.

use std::path::{Path, PathBuf};

use crate::json::Json;

/// The MCP resource URI under which the conventions file is served.
pub const URI: &str = "zap://conventions";

/// Default conventions file name, looked for at the repo root.
pub const DEFAULT_NAME: &str = "zap_mcp.md";

/// Accepted fallback location (checked after [`DEFAULT_NAME`]).
pub const FALLBACK_PATH: &str = ".zap/conventions.md";

/// MIME type for the served content.
pub const MIME: &str = "text/markdown";

/// Soft cap on the served body, in bytes. The content lands in an agent's
/// context window, so a very large file is truncated with a visible notice
/// rather than flooding the budget. 32 KiB is generous for house conventions
/// while staying well inside a typical context budget.
pub const SIZE_CAP: usize = 32 * 1024;

/// A discovered, loaded (and possibly truncated) conventions file.
pub struct Conventions {
    /// Repo-relative path the file was found at (for provenance in messages).
    pub rel_path: PathBuf,
    /// The served body — truncated to [`SIZE_CAP`] if the file was larger.
    pub body: String,
    /// Whether [`body`](Self::body) was truncated to fit the size cap.
    pub truncated: bool,
    /// The original file length in bytes, before any truncation.
    pub original_len: usize,
}

/// Discover and load the conventions file under `root`, if one exists.
///
/// Looks for [`DEFAULT_NAME`] first, then [`FALLBACK_PATH`]; the first that
/// reads successfully wins. Returns `None` when neither is present (or is
/// unreadable), so callers degrade gracefully.
pub fn discover(root: &Path) -> Option<Conventions> {
    for rel in [DEFAULT_NAME, FALLBACK_PATH] {
        let full = root.join(rel);
        // A directory or unreadable path simply means "not this candidate".
        if let Ok(raw) = std::fs::read_to_string(&full) {
            let original_len = raw.len();
            let (body, truncated) = cap(raw);
            return Some(Conventions {
                rel_path: PathBuf::from(rel),
                body,
                truncated,
                original_len,
            });
        }
    }
    None
}

/// Truncate `raw` to [`SIZE_CAP`] on a UTF-8 char boundary, appending a notice
/// when truncation occurs. Returns `(body, truncated)`.
fn cap(mut raw: String) -> (String, bool) {
    if raw.len() <= SIZE_CAP {
        return (raw, false);
    }
    let mut end = SIZE_CAP;
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    raw.truncate(end);
    raw.push_str(TRUNCATION_NOTICE);
    (raw, true)
}

const TRUNCATION_NOTICE: &str = "\n\n[... conventions truncated: the file exceeds the MCP \
    size budget; open it directly in the repository for the full text ...]\n";

impl Conventions {
    /// The MCP `resources/list` descriptor `{uri, name, description, mimeType}`.
    pub fn resource_descriptor(&self) -> Json {
        Json::obj(vec![
            ("uri", Json::str(URI)),
            ("name", Json::str("conventions")),
            (
                "description",
                Json::str(format!(
                    "This repository's coding conventions ({}){}.",
                    self.rel_path.display(),
                    if self.truncated {
                        ", truncated to the size budget"
                    } else {
                        ""
                    }
                )),
            ),
            ("mimeType", Json::str(MIME)),
        ])
    }
}

/// The message served (by both tool and resource read) when no conventions
/// file is present. Phrased so an agent knows the surface exists and how an org
/// opts in, without treating absence as an error.
pub fn absent_message() -> String {
    format!(
        "No conventions file present in this repository. An organization can add one at \
         `{DEFAULT_NAME}` (or `{FALLBACK_PATH}`) at the repo root to have its house coding \
         practices — naming, error handling, preferred modules, layering, testing — served to \
         agents automatically."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir() -> PathBuf {
        // Unique per call: a process-wide atomic counter plus the pid, so
        // parallel tests never collide on a coarse-resolution clock (two tests
        // sharing a dir would clobber each other's DEFAULT_NAME file and flake).
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "zap-conventions-{}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn write(dir: &Path, name: &str, body: &str) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn absent_when_no_file() {
        let dir = tmpdir();
        assert!(discover(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discovers_default_name_at_root() {
        let dir = tmpdir();
        write(
            &dir,
            DEFAULT_NAME,
            "# House style\nUse the `errors` Result helper.\n",
        );
        let c = discover(&dir).expect("conventions discovered");
        assert_eq!(c.rel_path, PathBuf::from(DEFAULT_NAME));
        assert!(c.body.contains("Result helper"));
        assert!(!c.truncated);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn falls_back_to_dot_zap_path() {
        let dir = tmpdir();
        write(&dir, FALLBACK_PATH, "# Conventions\nPrefer snake_case.\n");
        let c = discover(&dir).expect("fallback discovered");
        assert_eq!(c.rel_path, PathBuf::from(FALLBACK_PATH));
        assert!(c.body.contains("snake_case"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_name_wins_over_fallback() {
        let dir = tmpdir();
        write(&dir, DEFAULT_NAME, "ROOT FILE\n");
        write(&dir, FALLBACK_PATH, "DOT ZAP FILE\n");
        let c = discover(&dir).unwrap();
        assert_eq!(c.rel_path, PathBuf::from(DEFAULT_NAME));
        assert!(c.body.contains("ROOT FILE"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn large_file_is_truncated_with_notice() {
        let dir = tmpdir();
        let big = "x".repeat(SIZE_CAP + 10_000);
        write(&dir, DEFAULT_NAME, &big);
        let c = discover(&dir).unwrap();
        assert!(c.truncated);
        assert_eq!(c.original_len, SIZE_CAP + 10_000);
        assert!(c.body.len() <= SIZE_CAP + TRUNCATION_NOTICE.len());
        assert!(c.body.contains("conventions truncated"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let dir = tmpdir();
        // Multi-byte chars straddling the cap must not panic or split a char.
        let body = "é".repeat(SIZE_CAP); // 2 bytes each → well over the cap
        write(&dir, DEFAULT_NAME, &body);
        let c = discover(&dir).unwrap();
        assert!(c.truncated);
        // Valid UTF-8 round-trips (truncate on a char boundary).
        assert!(c
            .body
            .is_char_boundary(c.body.len() - TRUNCATION_NOTICE.len()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn descriptor_notes_truncation() {
        let plain = Conventions {
            rel_path: PathBuf::from(DEFAULT_NAME),
            body: "hi".into(),
            truncated: false,
            original_len: 2,
        };
        let d = plain.resource_descriptor();
        assert_eq!(d.get("uri").unwrap().as_str(), Some(URI));
        assert!(!d
            .get("description")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("truncated"));

        let cut = Conventions {
            rel_path: PathBuf::from(DEFAULT_NAME),
            body: "hi".into(),
            truncated: true,
            original_len: 999_999,
        };
        assert!(cut
            .resource_descriptor()
            .get("description")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("truncated"));
    }
}
