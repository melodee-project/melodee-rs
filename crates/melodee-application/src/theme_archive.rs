//! Bounded ZIP decoding shared by native theme uploads and legacy migration.
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};
use zip::{CompressionMethod, ZipArchive};

pub const MAX_THEME_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024;

pub const MAX_THEME_ENTRIES: usize = 256;
pub const MAX_THEME_ASSET_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_THEME_EXPANDED_BYTES: u64 = 100 * 1024 * 1024;

#[must_use]
pub fn safe_theme_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 1000
        && !name.starts_with('/')
        && !name.contains('\\')
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn read_declared(reader: impl Read, declared: u64) -> Result<Vec<u8>, &'static str> {
    if declared == 0 || declared > MAX_THEME_ASSET_BYTES {
        return Err("Theme archive entry exceeds the size limit");
    }
    let mut content = Vec::with_capacity(
        usize::try_from(declared).map_err(|_| "Theme archive entry is too large")?,
    );
    reader
        .take(declared + 1)
        .read_to_end(&mut content)
        .map_err(|_| "Theme archive entry could not be read")?;
    if content.len() as u64 != declared {
        return Err("Theme archive entry size does not match its declaration");
    }
    Ok(content)
}

/// Reads at most the compressed upload limit plus a sentinel byte, including
/// files whose advertised length changes while the importer is reading them.
pub fn read_theme_archive(reader: impl Read) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_THEME_ARCHIVE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_THEME_ARCHIVE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Theme archive exceeds the compressed size limit",
        ));
    }
    Ok(bytes)
}

/// Reads regular entries only, with both declared and actual expanded-size bounds.
/// Callers apply their protocol-specific metadata and asset-type contracts.
pub fn read_theme_entries(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, &'static str> {
    if bytes.len() as u64 > MAX_THEME_ARCHIVE_BYTES {
        return Err("Theme archive exceeds the compressed size limit");
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| "Invalid ZIP archive")?;
    if archive.is_empty() || archive.len() > MAX_THEME_ENTRIES {
        return Err("Theme archive contains an invalid number of entries");
    }
    let mut expanded = 0_u64;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| "Invalid ZIP archive entry")?;
        if entry.encrypted() {
            return Err("Encrypted theme archives are not supported");
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err("Unsupported ZIP compression method");
        }
        if entry.is_dir() {
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 != 0o100_000)
        {
            return Err("Theme archive links and special files are not allowed");
        }
        let name = entry
            .name()
            .map_err(|_| "Invalid ZIP archive entry")?
            .into_owned();
        if entry.enclosed_name().is_none() || !safe_theme_entry_name(&name) {
            return Err("Theme archive contains an unsafe path");
        }
        let declared = entry.size();
        expanded = expanded
            .checked_add(declared)
            .ok_or("Theme archive expanded size is invalid")?;
        if expanded > MAX_THEME_EXPANDED_BYTES {
            return Err("Theme archive exceeds the expanded size limit");
        }
        let content = read_declared(entry, declared)?;
        if entries.insert(name, content).is_some() {
            return Err("Theme archive contains duplicate paths");
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use std::io::Write;

    #[test]
    fn archive_declared_expansion_is_bounded_before_decompression() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "theme.css",
            zip::write::SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated),
        )
        .expect("entry");
        zip.write_all(&[b'x'; 1024]).expect("content");
        let valid = zip.finish().expect("archive").into_inner();
        assert_eq!(
            read_theme_entries(&valid).expect("valid")["theme.css"].len(),
            1024
        );
        let central = valid
            .windows(4)
            .position(|value| value == b"PK\x01\x02")
            .expect("directory");
        for declared in [1_u32, MAX_THEME_ASSET_BYTES as u32 + 1] {
            let mut corrupt = valid.clone();
            corrupt[central + 24..central + 28].copy_from_slice(&declared.to_le_bytes());
            assert!(read_theme_entries(&corrupt).is_err());
        }
    }

    #[test]
    fn archive_input_is_bounded_even_when_reader_never_ends() {
        assert_eq!(
            read_theme_archive(Cursor::new(b"abc")).expect("small"),
            b"abc"
        );
        assert!(read_theme_archive(std::io::repeat(0)).is_err());
    }

    use super::*;
    #[test]
    fn declared_and_actual_sizes_are_both_enforced() {
        assert_eq!(read_declared(Cursor::new(b"abc"), 3), Ok(b"abc".to_vec()));
        assert!(read_declared(Cursor::new(b"abc"), 2).is_err());
        assert!(read_declared(Cursor::new(b"abc"), 4).is_err());
        assert!(read_declared(std::io::empty(), MAX_THEME_ASSET_BYTES + 1).is_err());
    }
}
