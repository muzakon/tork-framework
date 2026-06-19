//! Password hashing (Argon2) and JWT signing/verification.
//!
//! Both are external crates. The framework provides the `BearerToken` extractor
//! and the dependency system; everything in this file is ordinary application
//! code you would write the same way in any Rust web app.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use tork::Error;

use crate::config::AuthConfig;

/// Hashes a plaintext password with Argon2 and a fresh random salt.
pub fn hash_password(password: &str) -> tork::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| Error::internal(format!("failed to hash password: {error}")))
}

/// Returns `true` if `password` matches the stored Argon2 `hash`.
pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// The claims encoded in the JWT.
///
/// `sub` is the user id, `exp` is the expiry as a Unix timestamp (jsonwebtoken
/// checks it automatically), and `scopes` carries the permissions granted.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    pub scopes: Vec<String>,
}

/// Signs a JWT for `user_id` with the given `scopes`, expiring after the
/// configured time-to-live.
pub fn encode_token(config: &AuthConfig, user_id: i64, scopes: Vec<String>) -> tork::Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let exp = now + config.access_token_ttl_minutes * 60;

    let claims = Claims {
        sub: user_id.to_string(),
        exp: exp as usize,
        scopes,
    };

    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(config.jwt_secret.expose().as_bytes()),
    )
    .map_err(|error| Error::internal(format!("failed to sign token: {error}")))
}

/// Verifies a JWT's signature and expiry and returns its claims.
///
/// A bad signature, an expired token, or malformed claims all return
/// `401 Unauthorized`.
pub fn decode_token(config: &AuthConfig, token: &str) -> tork::Result<Claims> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.jwt_secret.expose().as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .map(|data| data.claims)
    .map_err(|_| Error::unauthorized("invalid or expired token"))
}
