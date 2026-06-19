//! An in-memory user store and a repository over it.
//!
//! A real application would back this with a database (see the ORM guides). The
//! shape is what matters: passwords are stored only as Argon2 hashes, and each
//! user carries a role and a set of scopes used for authorization.

use std::collections::HashMap;
use std::sync::Arc;

use tork::{Error, Inject};

use crate::security::hash_password;

/// A stored user. The password is kept only as an Argon2 hash.
#[derive(Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub disabled: bool,
}

/// An in-memory user store, cheap to clone (the maps sit behind `Arc`).
#[derive(Clone)]
pub struct UserStore {
    by_id: Arc<HashMap<i64, User>>,
}

impl UserStore {
    /// Builds the store with a seeded set of users. The demo passwords are
    /// hashed once here, never stored in plaintext.
    pub fn seed() -> Self {
        let users = [
            User {
                id: 1,
                username: "ada".to_owned(),
                password_hash: hash_password("secret").expect("hash seed password"),
                role: "admin".to_owned(),
                scopes: vec!["users:read".to_owned(), "users:write".to_owned()],
                disabled: false,
            },
            User {
                id: 2,
                username: "alan".to_owned(),
                password_hash: hash_password("secret2").expect("hash seed password"),
                role: "member".to_owned(),
                scopes: vec!["users:read".to_owned()],
                disabled: false,
            },
            User {
                id: 3,
                username: "dot".to_owned(),
                password_hash: hash_password("secret3").expect("hash seed password"),
                role: "member".to_owned(),
                scopes: vec!["users:read".to_owned()],
                disabled: true,
            },
        ];

        let by_id = users.into_iter().map(|user| (user.id, user)).collect();
        Self { by_id: Arc::new(by_id) }
    }

    fn find_by_id(&self, id: i64) -> Option<User> {
        self.by_id.get(&id).cloned()
    }

    fn find_by_username(&self, username: &str) -> Option<User> {
        self.by_id.values().find(|user| user.username == username).cloned()
    }
}

/// The data-access layer for users. `#[derive(Inject)]` resolves the shared
/// `Arc<UserStore>` from the app on each request.
#[derive(Inject)]
pub struct UserRepository {
    store: Arc<UserStore>,
}

impl UserRepository {
    /// Looks up a user by username, for the login endpoint.
    pub fn find_by_username(&self, username: &str) -> Option<User> {
        self.store.find_by_username(username)
    }

    /// Looks up the user a verified token refers to. A token that names a user
    /// who no longer exists is treated as unauthorized.
    pub fn find_by_id(&self, id: i64) -> tork::Result<User> {
        self.store
            .find_by_id(id)
            .ok_or_else(|| Error::unauthorized("the token refers to an unknown user"))
    }
}
