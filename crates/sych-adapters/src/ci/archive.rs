//! Reading one report out of a CI artifact zip, in memory: nothing is written to
//! the host, every entry name is checked, decompression stops at a byte cap and the CRC must
//! match. Stored and deflate entries only; zip64, encryption and symlinks are refused.

use std::io::Read;

use flate2::Crc;
use flate2::read::DeflateDecoder;

const EOCD: u32 = 0x0605_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;
const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_entries: usize,
    /// Decompressed bytes of the one entry read.
    pub max_entry_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 1_000,
            max_entry_bytes: 32 << 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArchiveError {
    #[error("not a readable zip: {0}")]
    Malformed(String),
    #[error("unsupported zip feature: {0}")]
    Unsupported(String),
    #[error("entry name `{0}` could escape the staging directory")]
    PathRejected(String),
    #[error("archive over its bounds: {0}")]
    TooLarge(String),
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn bad(what: &str) -> ArchiveError {
    ArchiveError::Malformed(what.into())
}

/// A name that stays inside a staging directory on every platform.
fn safe(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.contains(':')
        && name.split('/').all(|part| part != "..")
}

struct Entry {
    name: String,
    method: u16,
    crc: u32,
    compressed: u32,
    local: u32,
}

fn central_directory(zip: &[u8], limits: Limits) -> Result<Vec<Entry>, ArchiveError> {
    let tail = zip.len().saturating_sub(22 + 65_535);
    let eocd = (tail..zip.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(zip, i) == Some(EOCD))
        .ok_or_else(|| bad("no end of central directory"))?;
    let count = u16_at(zip, eocd + 10).ok_or_else(|| bad("short directory"))?;
    let offset = u32_at(zip, eocd + 16).ok_or_else(|| bad("short directory"))?;
    if count == u16::MAX || offset == u32::MAX {
        return Err(ArchiveError::Unsupported("zip64".into()));
    }

    if usize::from(count) > limits.max_entries {
        return Err(ArchiveError::TooLarge(format!("{count} entries")));
    }

    let mut entries = Vec::new();
    let mut at = offset as usize;
    for _ in 0..count {
        if u32_at(zip, at) != Some(CENTRAL) {
            return Err(bad("broken central directory"));
        }

        let field = |o| u16_at(zip, at + o).ok_or_else(|| bad("short entry"));
        let word = |o| u32_at(zip, at + o).ok_or_else(|| bad("short entry"));
        let (flags, method) = (field(8)?, field(10)?);
        let (name_len, extra, comment) = (field(28)?, field(30)?, field(32)?);
        let (compressed, size, local) = (word(20)?, word(24)?, word(42)?);
        let name = zip
            .get(at + 46..at + 46 + usize::from(name_len))
            .ok_or_else(|| bad("short name"))?;
        let name = String::from_utf8(name.to_vec()).map_err(|_| bad("entry name is not UTF-8"))?;
        if !safe(&name) {
            return Err(ArchiveError::PathRejected(name));
        }

        if flags & 1 != 0 {
            return Err(ArchiveError::Unsupported(format!(
                "encrypted entry `{name}`"
            )));
        }

        if [compressed, size, local].contains(&u32::MAX) {
            return Err(ArchiveError::Unsupported("zip64".into()));
        }

        // Unix mode lives in the high half of the external attributes.
        if (word(38)? >> 16) & S_IFMT == S_IFLNK {
            return Err(ArchiveError::PathRejected(format!("{name} (symlink)")));
        }

        entries.push(Entry {
            name,
            method,
            crc: word(16)?,
            compressed,
            local,
        });
        at += 46 + usize::from(name_len) + usize::from(extra) + usize::from(comment);
    }
    Ok(entries)
}

/// The bytes of entry `wanted`, `None` when the archive has no such entry. Any unsafe name in
/// the archive rejects all of it.
pub fn read_entry(
    zip: &[u8],
    wanted: &str,
    limits: Limits,
) -> Result<Option<Vec<u8>>, ArchiveError> {
    let entries = central_directory(zip, limits)?;
    let Some(entry) = entries.iter().find(|e| e.name == wanted) else {
        return Ok(None);
    };
    let at = entry.local as usize;
    if u32_at(zip, at) != Some(LOCAL) {
        return Err(bad("broken local header"));
    }

    let name_len = u16_at(zip, at + 26).ok_or_else(|| bad("short local header"))?;
    let extra = u16_at(zip, at + 28).ok_or_else(|| bad("short local header"))?;
    let start = at + 30 + usize::from(name_len) + usize::from(extra);
    let data = zip
        .get(start..start + entry.compressed as usize)
        .ok_or_else(|| bad("entry data past the end"))?;
    let cap = limits.max_entry_bytes;
    let mut out = Vec::new();
    match entry.method {
        0 => data.take(cap + 1).read_to_end(&mut out),
        8 => DeflateDecoder::new(data)
            .take(cap + 1)
            .read_to_end(&mut out),
        m => return Err(ArchiveError::Unsupported(format!("compression method {m}"))),
    }
    .map_err(|e| bad(&format!("cannot inflate `{}`: {e}", entry.name)))?;
    if out.len() as u64 > cap {
        return Err(ArchiveError::TooLarge(format!(
            "`{}` inflates past {cap} bytes",
            entry.name
        )));
    }

    let mut crc = Crc::new();
    crc.update(&out);
    if crc.sum() != entry.crc {
        return Err(bad(&format!("CRC mismatch in `{}`", entry.name)));
    }

    Ok(Some(out))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::DeflateEncoder;

    use super::*;

    /// A zip of `(name, bytes, deflate, unix mode)` entries, as an artifact upload writes it.
    pub(crate) fn zip(entries: &[(&str, &[u8], bool, u32)]) -> Vec<u8> {
        let (mut out, mut central) = (Vec::new(), Vec::new());
        for (name, bytes, deflate, mode) in entries {
            let mut crc = Crc::new();
            crc.update(bytes);
            let data = if *deflate {
                let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
                e.write_all(bytes).expect("deflate");
                e.finish().expect("deflate")
            } else {
                bytes.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let offset = u32::try_from(out.len()).expect("small");
            let size = u32::try_from(bytes.len()).expect("small");
            let packed = u32::try_from(data.len()).expect("small");
            let n = u16::try_from(name.len()).expect("short");
            out.extend(LOCAL.to_le_bytes());
            out.extend([20, 0, 0, 0]);
            out.extend(method.to_le_bytes());
            out.extend([0; 4]);
            out.extend(crc.sum().to_le_bytes());
            out.extend(packed.to_le_bytes());
            out.extend(size.to_le_bytes());
            out.extend(n.to_le_bytes());
            out.extend([0, 0]);
            out.extend(name.as_bytes());
            out.extend(&data);
            central.extend(CENTRAL.to_le_bytes());
            central.extend([20, 3, 20, 0, 0, 0]);
            central.extend(method.to_le_bytes());
            central.extend([0; 4]);
            central.extend(crc.sum().to_le_bytes());
            central.extend(packed.to_le_bytes());
            central.extend(size.to_le_bytes());
            central.extend(n.to_le_bytes());
            central.extend([0; 8]);
            central.extend((mode << 16).to_le_bytes());
            central.extend(offset.to_le_bytes());
            central.extend(name.as_bytes());
        }
        let offset = u32::try_from(out.len()).expect("small");
        let count = u16::try_from(entries.len()).expect("few");
        out.extend(&central);
        out.extend(EOCD.to_le_bytes());
        out.extend([0; 4]);
        out.extend(count.to_le_bytes());
        out.extend(count.to_le_bytes());
        out.extend(u32::try_from(central.len()).expect("small").to_le_bytes());
        out.extend(offset.to_le_bytes());
        out.extend([0, 0]);
        out
    }

    const FILE: u32 = 0o100_644;

    #[test]
    fn reads_stored_and_deflated_entries() {
        let z = zip(&[
            ("a.txt", b"stored", false, FILE),
            ("dir/junit.xml", b"<x/>", true, FILE),
        ]);
        let limits = Limits::default();
        assert_eq!(
            read_entry(&z, "a.txt", limits),
            Ok(Some(b"stored".to_vec()))
        );
        assert_eq!(
            read_entry(&z, "dir/junit.xml", limits),
            Ok(Some(b"<x/>".to_vec()))
        );
        assert_eq!(read_entry(&z, "missing", limits), Ok(None));
    }

    #[test]
    fn traversal_symlinks_bombs_and_garbage_are_refused() {
        let limits = Limits::default();
        for name in ["../escape", "/etc/passwd", "a/../../b", "c:\\x", "a\\..\\b"] {
            let z = zip(&[(name, b"x", false, FILE), ("ok", b"y", false, FILE)]);
            assert!(
                matches!(
                    read_entry(&z, "ok", limits),
                    Err(ArchiveError::PathRejected(_))
                ),
                "{name}"
            );
        }
        let link = zip(&[("ok", b"target", false, S_IFLNK | 0o777)]);
        assert!(matches!(
            read_entry(&link, "ok", limits),
            Err(ArchiveError::PathRejected(_))
        ));
        let bomb = zip(&[("big", &vec![0u8; 4 << 20], true, FILE)]);
        assert!(bomb.len() < 64 << 10, "compresses well");
        let small = Limits {
            max_entry_bytes: 1 << 20,
            ..limits
        };
        assert!(matches!(
            read_entry(&bomb, "big", small),
            Err(ArchiveError::TooLarge(_))
        ));
        let many: Vec<(String, &[u8], bool, u32)> = (0..5)
            .map(|i| (format!("f{i}"), &b"x"[..], false, FILE))
            .collect();
        let refs: Vec<(&str, &[u8], bool, u32)> = many
            .iter()
            .map(|(n, b, d, m)| (n.as_str(), *b, *d, *m))
            .collect();
        let few = Limits {
            max_entries: 4,
            ..limits
        };
        assert!(matches!(
            read_entry(&zip(&refs), "f0", few),
            Err(ArchiveError::TooLarge(_))
        ));
        assert!(matches!(
            read_entry(b"not a zip", "x", limits),
            Err(ArchiveError::Malformed(_))
        ));
        let mut corrupt = zip(&[("a", b"payload", false, FILE)]);
        corrupt[30 + 1] ^= 0xff;
        assert!(matches!(
            read_entry(&corrupt, "a", limits),
            Err(ArchiveError::Malformed(_))
        ));
    }
}
