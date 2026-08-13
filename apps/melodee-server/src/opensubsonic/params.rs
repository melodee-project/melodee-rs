//! Merged OpenSubsonic request parameters (PRD-API-OS-001/003).
//!
//! Every endpoint accepts GET query parameters and urlencoded form POST
//! bodies (the advertised `formPost[1]` extension). Parameter binding in the
//! legacy server is ASP.NET model binding, which is case-insensitive, so keys
//! are stored lowercased. Repeated parameters (`id` lists for playlists and
//! play queues) keep all values in submission order.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Params {
    values: HashMap<String, Vec<String>>,
}

impl Params {
    pub fn add(&mut self, key: &str, value: &str) {
        self.values
            .entry(key.to_ascii_lowercase())
            .or_default()
            .push(value.to_string());
    }

    pub fn merge_query(&mut self, query: &str) {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            self.add(&decode_component(key), &decode_component(value));
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values
            .get(&key.to_ascii_lowercase())
            .and_then(|values| values.first())
            .map(String::as_str)
    }

    #[must_use]
    pub fn get_all(&self, key: &str) -> &[String] {
        self.values
            .get(&key.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    #[must_use]
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|value| value.parse().ok())
    }

    #[must_use]
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(|value| value.parse().ok())
    }

    #[must_use]
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key)
            .and_then(|value| match value.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            })
    }
}

fn decode_component(value: &str) -> String {
    let replaced = value.replace('+', " ");
    let bytes = replaced.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (
                hex_value(bytes.get(index + 1).copied()),
                hex_value(bytes.get(index + 2).copied()),
            )
        {
            out.push(hi * 16 + lo);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: Option<u8>) -> Option<u8> {
    match byte? {
        b'0'..=b'9' => Some(byte? - b'0'),
        b'a'..=b'f' => Some(byte? - b'a' + 10),
        b'A'..=b'F' => Some(byte? - b'A' + 10),
        _ => None,
    }
}

/// Lowercase hex encoding for md5 token comparison (the captured contract
/// compares case-insensitively, so we normalize both sides).
#[must_use]
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Hex decode used by the `enc:` password form. Returns `None` on malformed
/// input so callers can fall through to the captured auth failure.
#[must_use]
pub fn hex_decode(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let hi = hex_value(Some(bytes[index]))?;
        let lo = hex_value(Some(bytes[index + 1]))?;
        out.push(hi * 16 + lo);
        index += 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn query_parsing_decodes_and_keeps_repeats_in_order() {
        let mut params = Params::default();
        params.merge_query("u=Phase0&ID=a&id=b&empty=&encoded=%2Frest%2Fping%3Fx%3D1");
        assert_eq!(params.get("u"), Some("Phase0"));
        assert_eq!(params.get_all("id"), &["a".to_string(), "b".to_string()]);
        assert_eq!(params.get("empty"), Some(""));
        assert_eq!(params.get("encoded"), Some("/rest/ping?x=1"));
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(hex_decode("DEADbeef"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(hex_decode("abc"), None);
        assert_eq!(hex_decode("zz"), None);
    }
}
