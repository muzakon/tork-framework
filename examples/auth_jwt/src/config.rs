//! Application configuration, including the secret used to sign JWTs.

use tork::{settings, SecretString};

/// Authentication settings, loaded from the environment at startup.
///
/// The `jwt_secret` is read as a secret, so it is masked in logs and is never
/// serialized. Set it with the `AUTH_JWT_SECRET` environment variable in
/// production. The default exists only so the example runs without setup.
#[settings(prefix = "AUTH")]
pub struct AuthConfig {
    /// The HMAC key used to sign and verify JWTs.
    #[setting(secret, default = "dev-only-change-me")]
    pub jwt_secret: SecretString,
    /// How long an access token stays valid, in minutes.
    #[setting(default = 30, ge = 1, le = 1440)]
    pub access_token_ttl_minutes: u64,
}
