//! Content integrity for packages (D-0052).
//!
//! Integrity in the decentralized model comes from a content hash, not a
//! central registry. We hash a module's *tree* into a single digest and store
//! it in `zap.lock` as `zap1:<hex>`. The set of `(name, version) -> checksum`
//! rows is the project's slice of the checksum database.
//!
//! SHA-256 is implemented here directly rather than pulling the
//! `sha2`/`digest`/`generic-array` crate chain into a workspace whose only
//! other external dependency is `inkwell`; the implementation is checked
//! against the standard NIST test vectors in the unit tests below.

use std::fs;
use std::io;
use std::path::Path;

/// The checksum scheme prefix stored in the lockfile.
pub const PREFIX: &str = "zap1:";

const H0: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// The SHA-256 digest of `bytes`, as 32 raw bytes.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = H0;

    // Pre-processing: append 0x80, pad with zeros to 56 mod 64, then the
    // 64-bit big-endian bit length.
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut msg = bytes.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    for chunk in msg.chunks_exact(64) {
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Lowercase hex of a byte slice.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// The `zap1:` checksum of a single blob of bytes.
pub fn checksum_bytes(bytes: &[u8]) -> String {
    format!("{PREFIX}{}", hex(&sha256(bytes)))
}

/// The canonical content checksum of a module directory tree.
///
/// Every regular file under `root` contributes a line
/// `"<relpath>\0<hex sha256 of its bytes>\n"`; the lines are sorted by relpath
/// (byte order) and hashed, so the digest is independent of walk order and of
/// the absolute location of the tree. (This is the Go `h1:` idea reduced to a
/// flat hex digest.) `.git`, `target`, and `zenv` are skipped so a module's
/// own build/VCS artifacts never perturb its identity.
pub fn checksum_tree(root: &Path) -> io::Result<String> {
    let mut lines: Vec<String> = Vec::new();
    collect(root, root, &mut lines)?;
    lines.sort();
    let blob = lines.concat();
    Ok(checksum_bytes(blob.as_bytes()))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            if matches!(name.as_ref(), ".git" | "target" | "zenv") {
                continue;
            }
            collect(root, &entry.path(), out)?;
        } else if ty.is_file() {
            let bytes = fs::read(entry.path())?;
            let rel = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(&entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            out.push(format!("{rel}\0{}\n", hex(&sha256(&bytes))));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // NIST FIPS 180-2 standard vectors.
    #[test]
    fn empty_string() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn abc() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn two_block() {
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn long_message_crosses_block_boundary_in_length_pad() {
        // 56 bytes forces the length to spill into a second padding block.
        let msg = vec![b'a'; 56];
        // Reference value computed with the standard algorithm.
        assert_eq!(
            hex(&sha256(&msg)),
            "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"
        );
    }

    #[test]
    fn checksum_bytes_has_prefix() {
        let c = checksum_bytes(b"hello");
        assert!(c.starts_with("zap1:"));
        assert_eq!(&c[5..], hex(&sha256(b"hello")));
    }
}
