//! Shared application state.

use std::collections::HashMap;
use std::sync::Arc;

use tork::{Error, Result};

use crate::models::user::UserOut;

/// The application's shared state.
///
/// State must be cheap to clone, because the [`State`](tork::State) extractor
/// clones it on each request. Here the user store is held behind an `Arc`, so a
/// clone is just a reference-count bump. In a real service this would hold
/// connection pools and other handles.
#[derive(Clone)]
pub struct AppState {
    users: Arc<HashMap<i64, UserOut>>,
}

impl AppState {
    /// Boots the application state.
    ///
    /// This stands in for the asynchronous setup a real service performs, such
    /// as opening a database pool. Here it seeds an in-memory user store.
    pub async fn boot() -> Result<Self> {
        let mut users = HashMap::new();
        users.insert(
            1,
            UserOut {
                id: 1,
                email: "ada@example.com".to_owned(),
                name: "Ada Lovelace".to_owned(),
            },
        );
        users.insert(
            2,
            UserOut {
                id: 2,
                email: "alan@example.com".to_owned(),
                name: "Alan Turing".to_owned(),
            },
        );

        Ok(Self {
            users: Arc::new(users),
        })
    }

    /// Looks up a user by id.
    ///
    /// # Errors
    ///
    /// Returns a `404 Not Found` error if no user has the given id.
    pub fn get_user(&self, id: i64) -> Result<UserOut> {
        self.users
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::not_found("user not found"))
    }
}
