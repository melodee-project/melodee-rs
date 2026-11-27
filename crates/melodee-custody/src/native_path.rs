//! Canonical reversible Linux root-path metadata. This does not open a path or
//! prove the identity, confinement, mount topology, or ownership of a root.

use std::{
    ffi::OsString,
    io,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf},
};

const MAX_PATH_BYTES: usize = 131_072;
const MAX_DEPTH: usize = 256;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid native root path")
}

fn validate(path: &Path) -> io::Result<&[u8]> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_PATH_BYTES
        || !path.is_absolute()
        || bytes.contains(&0)
        || (bytes.len() > 1 && bytes.ends_with(b"/"))
        || bytes.windows(2).any(|pair| pair == b"//")
        || bytes
            .split(|byte| *byte == b'/')
            .any(|part| part == b"." || part == b"..")
        || path.components().count() > MAX_DEPTH
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(invalid());
    }
    Ok(bytes)
}

/// Encode a normalized absolute Linux path as lowercase raw-byte hex. The
/// caller must separately persist and check a `unix-hex-v1` format marker.
pub fn encode_native_root(path: &Path) -> io::Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = validate(path)?;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(out)
}

/// Decode exact raw bytes; alternate spellings and ambiguous paths fail closed.
pub fn decode_native_root(encoded: &str) -> io::Result<PathBuf> {
    if encoded.is_empty()
        || encoded.len() > MAX_PATH_BYTES * 2
        || encoded.len() % 2 != 0
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid());
    }
    let digit = |byte: u8| {
        if byte.is_ascii_digit() {
            byte - b'0'
        } else {
            byte - b'a' + 10
        }
    };
    let bytes: Vec<u8> = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| digit(pair[0]) * 16 + digit(pair[1]))
        .collect();
    let path = PathBuf::from(OsString::from_vec(bytes));
    validate(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    #[test]
    fn raw_non_unicode_root_round_trips_and_ambiguous_paths_fail_closed() {
        let path = PathBuf::from(OsString::from_vec(b"/owned/\xffroot".to_vec()));
        let encoded = encode_native_root(&path).expect("encode native bytes");
        assert_eq!(
            decode_native_root(&encoded).expect("decode native bytes"),
            path
        );
        assert!(encode_native_root(Path::new("/owned/../other")).is_err());
        assert!(encode_native_root(Path::new("/owned//other")).is_err());
        assert!(encode_native_root(Path::new("/owned/./other")).is_err());
        assert!(encode_native_root(Path::new("/owned/")).is_err());
        assert!(decode_native_root("2F").is_err());
        assert!(decode_native_root("2f0").is_err());
        assert!(decode_native_root("2f00").is_err());
        assert!(decode_native_root(&"61".repeat(MAX_PATH_BYTES + 1)).is_err());
    }
}
