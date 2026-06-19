//! A small example of JWT authentication and authorization on Tork.
//!
//! The flow: a client posts a username and password to `/token`, the server
//! verifies the password (Argon2) and returns a signed JWT, and protected
//! endpoints read the token through the `BearerToken` extractor and a
//! `#[tork::dependency]` guard that decodes and verifies it.

pub mod auth;
pub mod config;
pub mod routers;
pub mod schemas;
pub mod security;
pub mod users;

pub use config::AuthConfig;
pub use users::UserStore;
