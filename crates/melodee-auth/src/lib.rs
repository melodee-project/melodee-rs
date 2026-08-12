//! Secret-safe native authentication primitives (ADR-0124).
//!
//! PostgreSQL owns session state and rotation transactions. This crate owns
//! legacy password verification and irreversible refresh-token digestion.

#![forbid(unsafe_code)]

use std::fmt;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

const JWT_ISSUER: &str = "MelodeeApi";
const JWT_AUDIENCE: &str = "MelodeeClient";
const JWT_HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;
const COOKIE_VERSION: &str = "v1";
const COOKIE_SIGNING_DOMAIN: &str = "melodee-cookie-v1";

/// Frozen native-v1 JWT claims.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeAccessClaims {
    pub aud: String,
    pub email: String,
    pub exp: i64,
    #[serde(rename = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/sid")]
    pub sid: Uuid,
    pub iat: i64,
    pub iss: String,
    pub nbf: i64,
    pub role: Vec<String>,
    pub unique_name: String,
    #[serde(rename = "urn:user:theme")]
    pub theme: String,
}

/// Minimal claims carried in the browser-only native authentication cookie.
///
/// The cookie is intentionally a session cookie at the HTTP layer. The
/// embedded expiry bounds replay if a browser or intermediary retains it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeCookieClaims {
    pub public_api_id: Uuid,
    pub iat: i64,
    pub exp: i64,
}

/// Native HS256 token failures. Diagnostics deliberately carry no token data.
#[derive(Debug, Error)]
pub enum AccessTokenError {
    #[error("invalid native access token")]
    Invalid,
}

/// Issues the captured native-v1 HS256 token shape.
///
/// # Errors
/// Returns a secret-safe error when serialization or signing fails.
pub fn issue_access_token(
    signing_key: &SecretString,
    claims: &NativeAccessClaims,
) -> Result<String, AccessTokenError> {
    if !valid_claim_contract(claims) || signing_key.expose_secret().len() < 32 {
        return Err(AccessTokenError::Invalid);
    }
    let header = URL_SAFE_NO_PAD.encode(JWT_HEADER);
    let payload = serde_json::to_vec(claims).map_err(|_| AccessTokenError::Invalid)?;
    let payload = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{header}.{payload}");
    let signature = sign(signing_key, signing_input.as_bytes())?;
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

/// Generates an opaque 64-byte refresh token (88 base64 characters) using the
/// operating system CSPRNG, matching the captured native structure.
///
/// # Errors
/// Returns a secret-safe error if the OS random source is unavailable.
pub fn generate_refresh_token() -> Result<SecretString, AccessTokenError> {
    let mut bytes = [0_u8; 64];
    getrandom::fill(&mut bytes).map_err(|_| AccessTokenError::Invalid)?;
    Ok(SecretString::new(
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

/// Generates the captured 32-byte URL-safe password-reset token.
pub fn generate_password_reset_token() -> Result<SecretString, AccessTokenError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| AccessTokenError::Invalid)?;
    Ok(SecretString::new(URL_SAFE_NO_PAD.encode(bytes)))
}

/// Validates HS256, issuer, audience, not-before, and expiry with zero skew.
/// It performs no database lookup: native-v1 access JWTs are stateless.
///
/// # Errors
/// All parse, signature, and lifetime failures collapse to one secret-safe error.
pub fn validate_access_token(
    signing_key: &SecretString,
    token: &str,
    now_unix: i64,
) -> Result<NativeAccessClaims, AccessTokenError> {
    if token.len() > 16_384 || signing_key.expose_secret().len() < 32 {
        return Err(AccessTokenError::Invalid);
    }
    let mut parts = token.split('.');
    let header = parts.next().ok_or(AccessTokenError::Invalid)?;
    let payload = parts.next().ok_or(AccessTokenError::Invalid)?;
    let signature = parts.next().ok_or(AccessTokenError::Invalid)?;
    if parts.next().is_some() {
        return Err(AccessTokenError::Invalid);
    }
    let decoded_header = URL_SAFE_NO_PAD
        .decode(header)
        .map_err(|_| AccessTokenError::Invalid)?;
    let header_value = serde_json::from_slice::<serde_json::Value>(&decoded_header)
        .map_err(|_| AccessTokenError::Invalid)?;
    if header_value.get("alg").and_then(|value| value.as_str()) != Some("HS256")
        || header_value.get("typ").and_then(|value| value.as_str()) != Some("JWT")
    {
        return Err(AccessTokenError::Invalid);
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| AccessTokenError::Invalid)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key.expose_secret().as_bytes())
        .map_err(|_| AccessTokenError::Invalid)?;
    mac.update(format!("{header}.{payload}").as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| AccessTokenError::Invalid)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| AccessTokenError::Invalid)?;
    let claims = serde_json::from_slice::<NativeAccessClaims>(&payload)
        .map_err(|_| AccessTokenError::Invalid)?;
    if !valid_claim_contract(&claims)
        || claims.nbf > now_unix
        || claims.iat > now_unix
        || claims.exp <= now_unix
    {
        return Err(AccessTokenError::Invalid);
    }
    Ok(claims)
}

/// Issues an opaque, domain-separated HMAC browser-session value.
///
/// This is not an access JWT and is never accepted by bearer middleware.
///
/// # Errors
/// Returns a secret-safe error for invalid claims or serialization/signing
/// failures.
pub fn issue_cookie_session(
    signing_key: &SecretString,
    claims: &NativeCookieClaims,
) -> Result<String, AccessTokenError> {
    if signing_key.expose_secret().len() < 32
        || claims.public_api_id.is_nil()
        || claims.iat >= claims.exp
    {
        return Err(AccessTokenError::Invalid);
    }
    let payload = serde_json::to_vec(claims).map_err(|_| AccessTokenError::Invalid)?;
    let payload = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{COOKIE_SIGNING_DOMAIN}.{payload}");
    let signature = sign(signing_key, signing_input.as_bytes())?;
    Ok(format!(
        "{COOKIE_VERSION}.{payload}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

/// Validates a browser-session value with zero lifetime clock skew.
///
/// # Errors
/// All parse, signature, and lifetime failures collapse to one secret-safe
/// error.
pub fn validate_cookie_session(
    signing_key: &SecretString,
    value: &str,
    now_unix: i64,
) -> Result<NativeCookieClaims, AccessTokenError> {
    if value.len() > 4096 || signing_key.expose_secret().len() < 32 {
        return Err(AccessTokenError::Invalid);
    }
    let mut parts = value.split('.');
    if parts.next() != Some(COOKIE_VERSION) {
        return Err(AccessTokenError::Invalid);
    }
    let payload = parts.next().ok_or(AccessTokenError::Invalid)?;
    let signature = parts.next().ok_or(AccessTokenError::Invalid)?;
    if parts.next().is_some() {
        return Err(AccessTokenError::Invalid);
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| AccessTokenError::Invalid)?;
    let signing_input = format!("{COOKIE_SIGNING_DOMAIN}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key.expose_secret().as_bytes())
        .map_err(|_| AccessTokenError::Invalid)?;
    mac.update(signing_input.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| AccessTokenError::Invalid)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| AccessTokenError::Invalid)?;
    let claims = serde_json::from_slice::<NativeCookieClaims>(&payload)
        .map_err(|_| AccessTokenError::Invalid)?;
    if claims.public_api_id.is_nil() || claims.iat > now_unix || claims.exp <= now_unix {
        return Err(AccessTokenError::Invalid);
    }
    Ok(claims)
}

fn valid_claim_contract(claims: &NativeAccessClaims) -> bool {
    claims.iss == JWT_ISSUER
        && claims.aud == JWT_AUDIENCE
        && claims.iat <= claims.nbf
        && claims.nbf < claims.exp
        && !claims.email.trim().is_empty()
        && !claims.unique_name.trim().is_empty()
}

fn sign(signing_key: &SecretString, input: &[u8]) -> Result<[u8; 32], AccessTokenError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key.expose_secret().as_bytes())
        .map_err(|_| AccessTokenError::Invalid)?;
    mac.update(input);
    Ok(mac.finalize().into_bytes().into())
}

/// An owned secret whose standard diagnostic representation is always redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a secret value without validating or transforming it.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Deliberately exposes the secret to a cryptographic boundary.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(<redacted>)")
    }
}

/// Supported imported password-hash representations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordHashAlgorithm {
    /// The 60-character bcrypt representation captured from the legacy user table.
    Bcrypt,
}

impl PasswordHashAlgorithm {
    /// Parses the stable persistence tag.
    #[must_use]
    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "bcrypt" => Some(Self::Bcrypt),
            _ => None,
        }
    }
}

/// Password verification failures that are safe to expose at an internal boundary.
#[derive(Debug, Error)]
pub enum PasswordVerificationError {
    /// The persisted hash algorithm is known but not yet supported by this binary.
    #[error("password hash algorithm is not supported")]
    UnsupportedAlgorithm,
    /// The persisted hash does not conform to its declared representation.
    #[error("persisted password hash is malformed")]
    MalformedHash,
}

/// Verifies a password against an imported hash.
///
/// The bcrypt implementation performs its derived-key comparison in constant
/// time. Callers must map both `false` and errors to the same public invalid-
/// credentials response to avoid account enumeration.
///
/// # Errors
/// Returns an error when the declared algorithm is unsupported or the stored
/// hash is malformed.
pub fn verify_password(
    password: &[u8],
    algorithm: PasswordHashAlgorithm,
    password_hash: &str,
) -> Result<bool, PasswordVerificationError> {
    match algorithm {
        PasswordHashAlgorithm::Bcrypt => bcrypt::verify(password, password_hash)
            .map_err(|_| PasswordVerificationError::MalformedHash),
    }
}

/// Produces a bcrypt replacement hash using cost 12, matching the imported
/// hash family while keeping clear password material out of persistence.
///
/// # Errors
/// Returns a secret-safe error when hashing fails.
pub fn hash_password(password: &[u8]) -> Result<String, PasswordVerificationError> {
    bcrypt::hash(password, 12).map_err(|_| PasswordVerificationError::MalformedHash)
}

/// A SHA-256 refresh-token digest. Debug output is always redacted.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct RefreshTokenDigest([u8; 32]);

impl RefreshTokenDigest {
    /// Computes the one-way digest persisted by the refresh-token store.
    #[must_use]
    pub fn from_token(token: &[u8]) -> Self {
        Self(Sha256::digest(token).into())
    }

    /// Borrows the fixed-length database representation.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Validates a database representation before treating it as a digest.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }
}

impl fmt::Debug for RefreshTokenDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RefreshTokenDigest(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn bcrypt_legacy_hash_verifies_without_rehashing() {
        let hash = bcrypt::hash("Mel0deeR0cks!", 4).expect("test hash");
        assert!(
            verify_password(b"Mel0deeR0cks!", PasswordHashAlgorithm::Bcrypt, &hash,)
                .expect("valid bcrypt hash")
        );
        assert!(
            !verify_password(b"wrong", PasswordHashAlgorithm::Bcrypt, &hash)
                .expect("valid bcrypt hash")
        );
    }

    #[test]
    fn digest_matches_legacy_sha256_contract_and_debug_is_redacted() {
        let digest = RefreshTokenDigest::from_token(b"phase0-expired-refresh-token-fixture");
        assert_eq!(digest.as_bytes().len(), 32);
        assert_eq!(format!("{digest:?}"), "RefreshTokenDigest(<redacted>)");
        assert!(!format!("{digest:?}").contains("phase0"));
        assert_eq!(
            RefreshTokenDigest::from_bytes(digest.as_bytes()),
            Some(digest)
        );
    }

    #[test]
    fn native_access_token_is_exactly_timed_and_signature_checked() {
        let key = SecretString::new("a sufficiently long synthetic jwt signing key".to_string());
        let claims = NativeAccessClaims {
            aud: JWT_AUDIENCE.to_string(),
            email: "fixture@example.test".to_string(),
            exp: 1_900,
            sid: Uuid::nil(),
            iat: 1_000,
            iss: JWT_ISSUER.to_string(),
            nbf: 1_000,
            role: vec!["Administrator".to_string()],
            unique_name: "fixture".to_string(),
            theme: "dark".to_string(),
        };
        let token = issue_access_token(&key, &claims).expect("issue");
        assert_eq!(
            validate_access_token(&key, &token, 1_000).expect("valid"),
            claims
        );
        assert!(validate_access_token(&key, &token, 1_900).is_err());
        assert!(
            validate_access_token(
                &SecretString::new("another sufficiently long jwt signing key".to_string()),
                &token,
                1_000,
            )
            .is_err()
        );
    }

    #[test]
    fn browser_cookie_is_domain_separated_tamper_evident_and_time_bounded() {
        let key = SecretString::new("a sufficiently long synthetic jwt signing key".to_string());
        let claims = NativeCookieClaims {
            public_api_id: Uuid::now_v7(),
            iat: 1_000,
            exp: 2_000,
        };
        let cookie = issue_cookie_session(&key, &claims).expect("issue cookie");
        assert_eq!(
            validate_cookie_session(&key, &cookie, 1_000).expect("valid cookie"),
            claims
        );
        assert!(validate_cookie_session(&key, &cookie, 2_000).is_err());

        let tampered = format!("{cookie}x");
        assert!(validate_cookie_session(&key, &tampered, 1_000).is_err());
        assert!(validate_access_token(&key, &cookie, 1_000).is_err());
    }

    #[test]
    fn refresh_tokens_have_the_captured_64_byte_entropy_shape() {
        let first = generate_refresh_token().expect("OS random source");
        let second = generate_refresh_token().expect("OS random source");
        assert_eq!(first.expose_secret().len(), 88);
        assert_ne!(first, second);
        assert!(!format!("{first:?}").contains(first.expose_secret()));
    }
}
