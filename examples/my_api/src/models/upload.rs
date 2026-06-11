//! Form and file upload models.

use tork::{FileBytes, FormModel, UploadFile, api_model};

/// A profile upload submitted as `multipart/form-data`.
///
/// Mixes a small in-memory file ([`FileBytes`]), a spooled file
/// ([`UploadFile`]), and validated text fields.
#[derive(FormModel)]
pub struct ProfileForm {
    /// The avatar image, capped at 2 MB and limited to common image types.
    #[file(max_size = "2MB", content_types = ["image/png", "image/jpeg"], sniff = true)]
    pub avatar: FileBytes,
    /// An optional larger attachment, spooled to disk past the memory threshold.
    #[file(max_size = "20MB")]
    pub attachment: Option<UploadFile>,
    /// The display name, between 2 and 40 characters.
    #[field(min_length = 2, max_length = 40)]
    pub display_name: String,
    /// Free-form tags; the field may repeat.
    pub tags: Vec<String>,
}

/// The result of an upload: the bytes received and the echoed display name.
#[api_model(rename_all = "camelCase")]
pub struct UploadOut {
    /// Size of the avatar in bytes.
    pub avatar_size: usize,
    /// Size of the attachment in bytes, if one was sent.
    pub attachment_size: Option<u64>,
    /// The submitted display name.
    pub display_name: String,
    /// The submitted tags.
    pub tags: Vec<String>,
}

/// A login submitted as `application/x-www-form-urlencoded`.
#[api_model]
pub struct LoginForm {
    /// The account username.
    pub username: String,
    /// The account password.
    pub password: String,
}

/// The result of a login attempt.
#[api_model]
pub struct LoginOut {
    /// The authenticated username.
    pub username: String,
}
