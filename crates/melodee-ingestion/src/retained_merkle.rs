//! Versioned, bounded change-detection checkpoints for retained sources.
//!
//! Checkpoints hold only cryptographic metadata. They do not assert that the
//! source remained unchanged between chunks or replace the required second
//! observation and source-retention protocol.

use std::path::Path;

use crate::acquisition::{AcquisitionError, hash_custody_path};

pub(crate) const RETAINED_CHUNK_BYTES: usize = 64 * 1024;
const LEVELS: usize = 64;
const MAX_CHECKPOINT_BYTES: usize = 8192;
const ALGORITHM: &str = "blake3-binary-carry";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RetainedMerkleCheckpoint {
    pub(crate) format_version: u8,
    pub(crate) algorithm: String,
    pub(crate) chunk_bytes: u32,
    pub(crate) file_length: u64,
    pub(crate) chunk_count: u64,
    /// Binary-carry peaks, at most one digest per level. These values are
    /// hashes, never raw media bytes.
    pub(crate) peaks: Vec<(u8, String)>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCheckpoint {
    format_version: u8,
    algorithm: String,
    chunk_bytes: u32,
    file_length: u64,
    chunk_count: u64,
    #[serde(deserialize_with = "bounded_peaks")]
    peaks: Vec<(u8, String)>,
}

fn bounded_peaks<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<(u8, String)>, D::Error> {
    struct PeaksVisitor;
    impl<'de> serde::de::Visitor<'de> for PeaksVisitor {
        type Value = Vec<(u8, String)>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("at most 64 hash peaks")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut input: A,
        ) -> Result<Self::Value, A::Error> {
            let mut peaks = Vec::new();
            while let Some(peak) = input.next_element()? {
                if peaks.len() == LEVELS {
                    return Err(serde::de::Error::custom("too many hash peaks"));
                }
                peaks.push(peak);
            }
            Ok(peaks)
        }
    }
    deserializer.deserialize_seq(PeaksVisitor)
}

impl RetainedMerkleCheckpoint {
    pub(crate) fn decode(raw: &str) -> Result<Self, AcquisitionError> {
        if raw.len() > MAX_CHECKPOINT_BYTES {
            return Err(invalid("retained checkpoint exceeds byte limit"));
        }
        let wire: WireCheckpoint =
            serde_json::from_str(raw).map_err(|_| invalid("invalid retained checkpoint"))?;
        Ok(Self {
            format_version: wire.format_version,
            algorithm: wire.algorithm,
            chunk_bytes: wire.chunk_bytes,
            file_length: wire.file_length,
            chunk_count: wire.chunk_count,
            peaks: wire.peaks,
        })
    }
}

pub(crate) struct RetainedMerkleFile {
    file_length: u64,
    chunk_count: u64,
    peaks: [Option<[u8; 32]>; LEVELS],
    resumed: bool,
}

fn invalid(message: &str) -> AcquisitionError {
    AcquisitionError::IncompleteReceipt(message.to_owned())
}

impl RetainedMerkleFile {
    pub(crate) fn new(file_length: u64) -> Self {
        Self {
            file_length,
            chunk_count: 0,
            peaks: [None; LEVELS],
            resumed: false,
        }
    }

    pub(crate) fn restore(checkpoint: &RetainedMerkleCheckpoint) -> Result<Self, AcquisitionError> {
        if checkpoint.format_version != 4
            || checkpoint.algorithm != ALGORITHM
            || checkpoint.chunk_bytes != RETAINED_CHUNK_BYTES as u32
        {
            return Err(invalid("unsupported retained checkpoint format"));
        }
        let max_chunks = checkpoint.file_length.div_ceil(RETAINED_CHUNK_BYTES as u64);
        if checkpoint.chunk_count > max_chunks || checkpoint.peaks.len() > LEVELS {
            return Err(invalid("retained Merkle checkpoint count is invalid"));
        }
        let mut file = Self::new(checkpoint.file_length);
        file.resumed = true;
        file.chunk_count = checkpoint.chunk_count;
        for (level, encoded) in &checkpoint.peaks {
            let level = usize::from(*level);
            if level >= LEVELS
                || checkpoint.chunk_count & (1_u64 << level) == 0
                || file.peaks[level].is_some()
                || encoded.len() != 64
                || !encoded
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(invalid("retained Merkle checkpoint peak is invalid"));
            }
            let mut value = [0_u8; 32];
            for (slot, pair) in value.iter_mut().zip(encoded.as_bytes().chunks_exact(2)) {
                let hex =
                    std::str::from_utf8(pair).map_err(|_| invalid("invalid digest encoding"))?;
                *slot =
                    u8::from_str_radix(hex, 16).map_err(|_| invalid("invalid digest encoding"))?;
            }
            file.peaks[level] = Some(value);
        }
        for level in 0..LEVELS {
            if file.peaks[level].is_some() != (file.chunk_count & (1_u64 << level) != 0) {
                return Err(invalid("retained Merkle checkpoint is incomplete"));
            }
        }
        Ok(file)
    }

    pub(crate) fn push(&mut self, index: u64, bytes: &[u8]) -> Result<(), AcquisitionError> {
        if index != self.chunk_count {
            return Err(invalid("retained chunk order changed"));
        }
        let offset = index
            .checked_mul(RETAINED_CHUNK_BYTES as u64)
            .ok_or_else(|| invalid("retained chunk offset overflow"))?;
        let remaining = self
            .file_length
            .checked_sub(offset)
            .ok_or_else(|| invalid("retained chunk exceeds file length"))?;
        let expected = remaining.min(RETAINED_CHUNK_BYTES as u64) as usize;
        if expected == 0 || bytes.len() != expected {
            return Err(invalid("retained chunk length changed"));
        }
        let mut leaf = blake3::Hasher::new();
        leaf.update(b"melodee-retained-file-chunk-v4\0");
        leaf.update(&index.to_le_bytes());
        leaf.update(&(bytes.len() as u64).to_le_bytes());
        leaf.update(bytes);
        let mut digest = *leaf.finalize().as_bytes();
        let mut level = 0;
        while let Some(left) = self.peaks[level].take() {
            let mut parent = blake3::Hasher::new();
            parent.update(b"melodee-retained-file-node-v4\0");
            parent.update(&(level as u64).to_le_bytes());
            parent.update(&left);
            parent.update(&digest);
            digest = *parent.finalize().as_bytes();
            level += 1;
            if level == LEVELS {
                return Err(invalid("retained Merkle tree overflow"));
            }
        }
        self.peaks[level] = Some(digest);
        self.chunk_count += 1;
        Ok(())
    }

    pub(crate) fn checkpoint(&self) -> RetainedMerkleCheckpoint {
        let peaks = self
            .peaks
            .iter()
            .enumerate()
            .filter_map(|(level, digest)| {
                digest.map(|digest| {
                    (
                        level as u8,
                        blake3::Hash::from_bytes(digest).to_hex().to_string(),
                    )
                })
            })
            .collect();
        RetainedMerkleCheckpoint {
            format_version: 4,
            algorithm: ALGORITHM.to_owned(),
            chunk_bytes: RETAINED_CHUNK_BYTES as u32,
            file_length: self.file_length,
            chunk_count: self.chunk_count,
            peaks,
        }
    }

    pub(crate) fn finish(&self, path: &Path) -> Result<String, AcquisitionError> {
        if self.resumed {
            return Err(invalid(
                "resumed checkpoint is only a change-detection hint",
            ));
        }
        if self.chunk_count != self.file_length.div_ceil(RETAINED_CHUNK_BYTES as u64) {
            return Err(invalid("retained Merkle file is incomplete"));
        }
        let mut root = blake3::Hasher::new();
        root.update(b"melodee-retained-file-root-v4\0");
        hash_custody_path(&mut root, path, false, self.file_length)?;
        root.update(&self.chunk_count.to_le_bytes());
        for (level, digest) in self.peaks.iter().enumerate().rev() {
            if let Some(digest) = digest {
                root.update(&(level as u64).to_le_bytes());
                root.update(digest);
            }
        }
        Ok(format!("v4:{}", root.finalize().to_hex()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;

    #[test]
    fn resumed_checkpoint_is_only_a_hint_and_cannot_seal_stale_bytes() {
        let length = RETAINED_CHUNK_BYTES * 3 + 17;
        let bytes: Vec<_> = (0..length).map(|index| (index % 251) as u8).collect();
        let mut uninterrupted = RetainedMerkleFile::new(length as u64);
        let mut before_crash = RetainedMerkleFile::new(length as u64);
        for (index, part) in bytes.chunks(RETAINED_CHUNK_BYTES).enumerate() {
            uninterrupted.push(index as u64, part).expect("full push");
            if index < 2 {
                before_crash.push(index as u64, part).expect("partial push");
            }
        }
        let checkpoint = serde_json::to_string(&before_crash.checkpoint()).expect("serialize");
        let recovered = RetainedMerkleCheckpoint::decode(&checkpoint).expect("deserialize");
        let mut resumed = RetainedMerkleFile::restore(&recovered).expect("restore");
        for (index, part) in bytes.chunks(RETAINED_CHUNK_BYTES).enumerate().skip(2) {
            resumed.push(index as u64, part).expect("resumed push");
        }
        let expected = uninterrupted
            .finish(Path::new("disc/track.flac"))
            .expect("proof");
        assert_eq!(resumed.checkpoint(), uninterrupted.checkpoint());
        assert!(
            resumed.finish(Path::new("disc/track.flac")).is_err(),
            "a resumed mixed-time prefix cannot be a fresh proof"
        );
        assert_ne!(
            uninterrupted
                .finish(Path::new("disc/other.flac"))
                .expect("other proof"),
            expected
        );
        let mut changed = bytes.clone();
        changed[RETAINED_CHUNK_BYTES + 7] ^= 1;
        let mut alternate = RetainedMerkleFile::new(length as u64);
        for (index, part) in changed.chunks(RETAINED_CHUNK_BYTES).enumerate() {
            alternate.push(index as u64, part).expect("changed push");
        }
        assert_ne!(
            alternate.checkpoint(),
            resumed.checkpoint(),
            "a fresh re-read detects mutation in the checkpointed prefix"
        );
        assert_ne!(
            alternate
                .finish(Path::new("disc/track.flac"))
                .expect("changed"),
            expected
        );
    }

    #[test]
    fn checkpoint_rejects_invalid_order_shape_and_incomplete_chunks() {
        let mut file = RetainedMerkleFile::new((RETAINED_CHUNK_BYTES + 1) as u64);
        assert!(file.push(1, &[0]).is_err());
        assert!(file.push(0, &[0]).is_err());
        file.push(0, &vec![3; RETAINED_CHUNK_BYTES]).expect("first");
        assert!(file.finish(Path::new("file")).is_err());
        let mut checkpoint = file.checkpoint();
        checkpoint.peaks.clear();
        assert!(RetainedMerkleFile::restore(&checkpoint).is_err());
        let mut checkpoint = file.checkpoint();
        checkpoint.peaks.push(checkpoint.peaks[0].clone());
        assert!(RetainedMerkleFile::restore(&checkpoint).is_err());
        let mut checkpoint = file.checkpoint();
        checkpoint.format_version = 5;
        assert!(RetainedMerkleFile::restore(&checkpoint).is_err());
        let mut checkpoint = file.checkpoint();
        checkpoint.chunk_bytes = 1024;
        assert!(RetainedMerkleFile::restore(&checkpoint).is_err());
        let mut checkpoint = file.checkpoint();
        checkpoint.algorithm = "other".to_owned();
        assert!(RetainedMerkleFile::restore(&checkpoint).is_err());
        assert!(RetainedMerkleCheckpoint::decode(&" ".repeat(MAX_CHECKPOINT_BYTES + 1)).is_err());
        let too_many = serde_json::json!({"format_version":4,"algorithm":ALGORITHM,"chunk_bytes":RETAINED_CHUNK_BYTES,"file_length":0,"chunk_count":0,"peaks":vec![(0,"a".repeat(64));65]}).to_string();
        assert!(too_many.len() < MAX_CHECKPOINT_BYTES);
        assert!(RetainedMerkleCheckpoint::decode(&too_many).is_err());
        file.push(1, &[4]).expect("final partial");
        assert!(file.push(2, &[5]).is_err());
        assert!(file.finish(Path::new("file")).is_ok());
        assert!(
            RetainedMerkleFile::new(0)
                .finish(Path::new("empty"))
                .is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn distinct_non_unicode_names_bind_distinct_file_roots() {
        use std::os::unix::ffi::OsStringExt;
        let first = std::ffi::OsString::from_vec(b"bad-\x80".to_vec());
        let second = std::ffi::OsString::from_vec(b"bad-\x81".to_vec());
        let mut file = RetainedMerkleFile::new(3);
        file.push(0, b"abc").expect("chunk");
        assert_ne!(
            file.finish(Path::new(&first)).expect("first"),
            file.finish(Path::new(&second)).expect("second")
        );
    }

    #[test]
    fn v4_leaf_and_parent_match_explicit_reference_preimages() {
        let a = vec![0x42; RETAINED_CHUNK_BYTES];
        let b = vec![0x93; RETAINED_CHUNK_BYTES];
        let reference_leaf = |index: u64, bytes: &[u8]| {
            let mut input = b"melodee-retained-file-chunk-v4\0".to_vec();
            input.extend_from_slice(&index.to_le_bytes());
            input.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            input.extend_from_slice(bytes);
            blake3::hash(&input)
        };
        let left = reference_leaf(0, &a);
        let right = reference_leaf(1, &b);
        let mut reference_node = b"melodee-retained-file-node-v4\0".to_vec();
        reference_node.extend_from_slice(&0_u64.to_le_bytes());
        reference_node.extend_from_slice(left.as_bytes());
        reference_node.extend_from_slice(right.as_bytes());
        let mut file = RetainedMerkleFile::new((a.len() + b.len()) as u64);
        file.push(0, &a).expect("first");
        assert_eq!(
            file.checkpoint().peaks,
            vec![(0, left.to_hex().to_string())]
        );
        file.push(1, &b).expect("second");
        assert_eq!(
            file.checkpoint().peaks,
            vec![(1, blake3::hash(&reference_node).to_hex().to_string())]
        );
    }
}
