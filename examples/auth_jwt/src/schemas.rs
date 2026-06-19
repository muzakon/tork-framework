//! Request and response models for the auth endpoints.

use tork::api_model;

/// The login request body.
#[api_model]
pub struct LoginInput {
    #[field(min_length = 1, max_length = 50)]
    pub username: String,
    #[field(min_length = 1, max_length = 200)]
    pub password: String,
}

/// The token response, following the usual `{ access_token, token_type }` shape.
#[api_model]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
}

/// The public view of a user.
#[api_model]
pub struct UserOut {
    pub id: i64,
    pub username: String,
    pub role: String,
}
