//! The deterministic package archive (design/0027-registry.md §2.1).
//!
//! The archive is a **transport container**, not the integrity identity: the
//! pinned value is always the `zap1:` *tree hash* recomputed over the unpacked
//! tree ([`crate::checksum`]), so a different-but-equivalent archive can never
//! cause a false mismatch. The archive carries only a **secondary** digest —
//! the SHA-256 of its bytes — used as a cheap pre-unpack transfer check (§2.2
//! step 3).
//!
//! The container is a minimal, deterministic **USTAR tar**: entries sorted by
//! path, fixed mode / mtime / uid / gid / owner, VCS and build junk excluded,
//! so the bytes are reproducible from the content alone. Compression is a
//! separate, swappable step: this cut ships the `Store` codec (no compression,
//! `.zap.tar`); a `zstd` codec (`.zap.tar.zst`) can drop in behind
//! [`Codec`] without touching the identity or the tests (see I#236 PR notes).

use std::fs;
use std::path::{Path, PathBuf};

use crate::checksum::{hex, sha256};
use crate::error::{Error, ErrorCode, Result};

/// A tar block is 512 bytes.
const BLOCK: usize = 512;

/// Directory names excluded from an archive (matches [`crate::checksum`] so the
/// archived tree and the hashed tree are the same set of files).
const EXCLUDE_DIRS: &[&str] = &[".git", "target", "zenv"];
/// Individual junk files excluded regardless of directory.
const EXCLUDE_FILES: &[&str] = &[".DS_Store"];

/// The archive compression codec. `Store` is the no-compression container used
/// by the #236 read path; the variant exists so a `zstd` codec can be added
/// without changing call sites or the archive identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// No compression — the archive bytes are the raw tar bytes (`.zap.tar`).
    Store,
}

impl Codec {
    /// The archive file extension for this codec.
    pub fn extension(self) -> &'static str {
        match self {
            Codec::Store => "zap.tar",
        }
    }

    /// The codec implied by an archive URL or `config.json` `dl` template,
    /// chosen purely from its extension. This is what keeps the archive format
    /// a server-side (`dl`) decision: `.zap.tar` is the current `Store` codec,
    /// and `.zap.tar.zst` will select a future `zstd` codec with no client
    /// change — until then it is a clear, forward-compatible error rather than
    /// a silent mis-read.
    pub fn from_dl(dl: &str) -> Result<Codec> {
        // Ignore any `{...}` placeholders / query string when matching the tail.
        let tail = dl.split(['?', '#']).next().unwrap_or(dl);
        if tail.ends_with(".zap.tar") {
            Ok(Codec::Store)
        } else if tail.ends_with(".zap.tar.zst") {
            Err(malformed(
                "this toolchain cannot read `.zap.tar.zst` (zstd) archives yet; \
                 the zstd codec lands with `zap publish` (#237)",
            ))
        } else {
            Err(malformed(format!(
                "unrecognized archive format in download template `{dl}` (expected a \
                 `.zap.tar` suffix)"
            )))
        }
    }
}

/// Build a deterministic archive of the package tree rooted at `root`.
///
/// Files are collected (excluding VCS/build/junk), sorted by their
/// forward-slash relative path, and written as USTAR entries with fixed
/// metadata, then the stream is terminated with the two zero blocks tar
/// requires. The result is byte-identical for identical content.
pub fn build(root: &Path, codec: Codec) -> Result<Vec<u8>> {
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    // Deterministic order: byte-sorted relative path.
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut tar = Vec::new();
    for (rel, abs) in &files {
        let bytes = fs::read(abs).map_err(|e| Error::io(abs.display().to_string(), e))?;
        write_entry(&mut tar, rel, &bytes)?;
    }
    // End-of-archive: two zero-filled blocks.
    tar.extend(std::iter::repeat(0u8).take(BLOCK * 2));

    match codec {
        Codec::Store => Ok(tar),
    }
}

/// The secondary transfer-check digest of archive bytes: lowercase hex SHA-256
/// (no `zap1:` prefix — this is the container digest, not the tree identity).
pub fn digest(archive: &[u8]) -> String {
    hex(&sha256(archive))
}

/// Unpack an archive (produced by [`build`]) into `dest`, which is created.
/// Entry paths are validated to stay within `dest` (no absolute paths, no
/// `..`), so a malicious archive cannot escape the destination.
pub fn unpack(archive: &[u8], dest: &Path, codec: Codec) -> Result<()> {
    let tar: &[u8] = match codec {
        Codec::Store => archive,
    };
    fs::create_dir_all(dest).map_err(|e| Error::io(dest.display().to_string(), e))?;

    let mut pos = 0;
    while pos + BLOCK <= tar.len() {
        let header = &tar[pos..pos + BLOCK];
        // Two consecutive zero blocks (or a zero block) end the archive.
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let name = read_str(&header[0..100]);
        let prefix = read_str(&header[345..345 + 155]);
        let full = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let size = read_octal(&header[124..124 + 12])? as usize;
        let typeflag = header[156];
        pos += BLOCK;

        let data_end = pos + size;
        if data_end > tar.len() {
            return Err(malformed("archive truncated: entry data past end"));
        }
        // Only regular files are emitted by `build`; accept '0' and '\0'.
        if matches!(typeflag, b'0' | 0) {
            let target = safe_join(dest, &full)?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| Error::io(parent.display().to_string(), e))?;
            }
            fs::write(&target, &tar[pos..data_end])
                .map_err(|e| Error::io(target.display().to_string(), e))?;
        }
        // Advance past the data, rounded up to a block boundary.
        pos = data_end.div_ceil(BLOCK) * BLOCK;
    }
    Ok(())
}

/// Recursively collect `(relpath, abspath)` of every archivable file.
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let rd = fs::read_dir(dir).map_err(|e| Error::io(dir.display().to_string(), e))?;
    for entry in rd {
        let entry = entry.map_err(|e| Error::io(dir.display().to_string(), e))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ty = entry
            .file_type()
            .map_err(|e| Error::io(entry.path().display().to_string(), e))?;
        if ty.is_dir() {
            if EXCLUDE_DIRS.contains(&name.as_ref()) {
                continue;
            }
            collect(root, &entry.path(), out)?;
        } else if ty.is_file() {
            if EXCLUDE_FILES.contains(&name.as_ref()) {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(&entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, entry.path()));
        }
    }
    Ok(())
}

/// Append one USTAR file entry (header + padded data) to `tar`.
fn write_entry(tar: &mut Vec<u8>, rel: &str, data: &[u8]) -> Result<()> {
    let mut header = [0u8; BLOCK];
    let (name, prefix) = split_name(rel)?;
    header[0..name.len()].copy_from_slice(name.as_bytes());
    // Fixed, reproducible metadata.
    write_octal(&mut header[100..108], 0o644, 7); // mode
    write_octal(&mut header[108..116], 0, 7); // uid
    write_octal(&mut header[116..124], 0, 7); // gid
    write_octal(&mut header[124..136], data.len() as u64, 11); // size
    write_octal(&mut header[136..148], 0, 11); // mtime = epoch
    header[156] = b'0'; // typeflag: regular file
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    if !prefix.is_empty() {
        header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    }
    // Checksum: the sum of all header bytes with the checksum field as spaces.
    for b in &mut header[148..156] {
        *b = b' ';
    }
    let sum: u32 = header.iter().map(|&b| b as u32).sum();
    // Six octal digits, a NUL, then a space (the canonical form).
    write_octal(&mut header[148..154], sum as u64, 5);
    header[154] = 0;
    header[155] = b' ';

    tar.extend_from_slice(&header);
    tar.extend_from_slice(data);
    let pad = (BLOCK - data.len() % BLOCK) % BLOCK;
    tar.extend(std::iter::repeat(0u8).take(pad));
    Ok(())
}

/// Split a relative path into USTAR `(name<=100, prefix<=155)`. Errors if the
/// path cannot fit — package trees are shallow, so this is a guard, not a path
/// we expect to hit.
fn split_name(rel: &str) -> Result<(&str, &str)> {
    if rel.len() <= 100 {
        return Ok((rel, ""));
    }
    // Find a split on a '/' so name<=100 and prefix<=155.
    if let Some(idx) = rel[..rel.len() - 100].rfind('/') {
        let prefix = &rel[..idx];
        let name = &rel[idx + 1..];
        if name.len() <= 100 && prefix.len() <= 155 {
            return Ok((name, prefix));
        }
    }
    Err(malformed(format!(
        "path too long for the archive format: `{rel}`"
    )))
}

/// Join `base` with an archive-relative path, rejecting escapes.
fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let mut out = base.to_path_buf();
    for comp in rel.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return Err(malformed(format!(
                "archive entry escapes destination: `{rel}`"
            )));
        }
        out.push(comp);
    }
    Ok(out)
}

/// Write `value` as a zero-padded octal string of `digits` digits followed by a
/// NUL into `field` (the standard tar numeric field encoding).
fn write_octal(field: &mut [u8], value: u64, digits: usize) {
    let s = format!("{value:0width$o}", width = digits);
    let bytes = s.as_bytes();
    field[..bytes.len()].copy_from_slice(bytes);
    field[bytes.len()] = 0;
}

/// Parse a NUL/space-terminated octal numeric field.
fn read_octal(field: &[u8]) -> Result<u64> {
    let s: String = field
        .iter()
        .take_while(|&&b| b != 0 && b != b' ')
        .map(|&b| b as char)
        .collect();
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(&s, 8).map_err(|_| malformed("invalid octal field in archive header"))
}

/// Read a NUL-terminated string field.
fn read_str(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn malformed(message: impl Into<String>) -> Error {
    Error::registry(ErrorCode::ZAP6105, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmp(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("zapc_arch-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(root: &Path) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("zap.toml"), "[package]\nname=\"x\"\n").unwrap();
        fs::write(root.join("src/lib.zap"), "def f(): pass\n").unwrap();
        fs::write(root.join("README.md"), "hi\n").unwrap();
    }

    #[test]
    fn build_is_byte_reproducible() {
        let a = tmp("repro-a");
        let b = tmp("repro-b");
        sample(&a);
        sample(&b);
        let ba = build(&a, Codec::Store).unwrap();
        let bb = build(&b, Codec::Store).unwrap();
        assert_eq!(ba, bb, "identical content must archive to identical bytes");
        assert_eq!(digest(&ba), digest(&bb));
        // Ends with two zero blocks.
        assert!(ba.len() >= BLOCK * 2);
        assert!(ba[ba.len() - BLOCK * 2..].iter().all(|&x| x == 0));
    }

    #[test]
    fn excluded_dirs_do_not_perturb_bytes() {
        let a = tmp("excl-a");
        let b = tmp("excl-b");
        sample(&a);
        sample(&b);
        fs::create_dir_all(b.join(".git")).unwrap();
        fs::write(b.join(".git/HEAD"), "ref: x\n").unwrap();
        fs::write(b.join(".DS_Store"), "junk").unwrap();
        assert_eq!(
            build(&a, Codec::Store).unwrap(),
            build(&b, Codec::Store).unwrap()
        );
    }

    #[test]
    fn round_trips_through_unpack() {
        let src = tmp("rt-src");
        let dst = tmp("rt-dst");
        sample(&src);
        let archive = build(&src, Codec::Store).unwrap();
        unpack(&archive, &dst, Codec::Store).unwrap();
        assert_eq!(
            fs::read_to_string(dst.join("src/lib.zap")).unwrap(),
            "def f(): pass\n"
        );
        assert_eq!(
            fs::read_to_string(dst.join("zap.toml")).unwrap(),
            "[package]\nname=\"x\"\n"
        );
        // The unpacked tree hashes identically to the original (the invariant
        // the whole scheme rests on).
        assert_eq!(
            crate::checksum::checksum_tree(&src).unwrap(),
            crate::checksum::checksum_tree(&dst).unwrap()
        );
    }

    #[test]
    fn codec_is_chosen_from_the_dl_template() {
        // `.zap.tar` (current) selects Store; `{name}`/`{vers}` placeholders and
        // a query string do not confuse the suffix match.
        assert_eq!(
            Codec::from_dl("file:///i/{name}/{name}-{vers}.zap.tar").unwrap(),
            Codec::Store
        );
        assert_eq!(
            Codec::from_dl("https://cdn/a-{vers}.zap.tar?token=x").unwrap(),
            Codec::Store
        );
        // `.zap.tar.zst` is recognized but not yet readable (forward-compatible
        // error, not a silent mis-read); an unknown suffix also errors.
        assert!(Codec::from_dl("https://cdn/a-{vers}.zap.tar.zst").is_err());
        assert!(Codec::from_dl("https://cdn/a.tgz").is_err());
    }

    #[test]
    fn unpack_rejects_path_escape() {
        // Hand-craft a header whose name is `../evil`.
        let mut bad = Vec::new();
        super::write_entry(&mut bad, "../evil", b"x").unwrap();
        bad.extend(std::iter::repeat(0u8).take(BLOCK * 2));
        let dst = tmp("escape");
        assert!(unpack(&bad, &dst, Codec::Store).is_err());
    }
}
