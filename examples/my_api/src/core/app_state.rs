//! Shared application state.

use std::collections::HashMap;
use std::sync::Arc;

use tork::{Error, Result};

use crate::models::order::OrderOut;
use crate::models::user::UserOut;

/// The application's shared state.
///
/// State must be cheap to clone, because the [`State`](tork::State) extractor
/// clones it on each request. Here every store is held behind an `Arc`, so a
/// clone is just a few reference-count bumps. In a real service this would hold
/// connection pools and other handles.
#[derive(Clone)]
pub struct AppState {
    users: Arc<HashMap<i64, UserOut>>,
    orders: Arc<HashMap<i64, Vec<OrderOut>>>,
    tokens: Arc<HashMap<String, i64>>,
}

impl AppState {
    /// Boots the application state.
    ///
    /// This stands in for the asynchronous setup a real service performs, such
    /// as opening a database pool. Here it seeds an in-memory data set.
    pub async fn boot() -> Result<Self> {
        let users = HashMap::from([
            (
                1,
                UserOut {
                    id: 1,
                    email: "ada@example.com".to_owned(),
                    name: "Ada Lovelace".to_owned(),
                },
            ),
            (
                2,
                UserOut {
                    id: 2,
                    email: "alan@example.com".to_owned(),
                    name: "Alan Turing".to_owned(),
                },
            ),
        ]);

        let orders = HashMap::from([(
            1,
            vec![
                OrderOut {
                    id: 1001,
                    user_id: 1,
                    total_cents: 4999,
                },
                OrderOut {
                    id: 1002,
                    user_id: 1,
                    total_cents: 1500,
                },
            ],
        )]);

        // Maps an opaque bearer token to a user id. A real service would verify a
        // signed token or look up a session instead.
        let tokens = HashMap::from([("ada-token".to_owned(), 1), ("alan-token".to_owned(), 2)]);

        Ok(Self {
            users: Arc::new(users),
            orders: Arc::new(orders),
            tokens: Arc::new(tokens),
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

    /// Returns the orders belonging to a user, or an empty list if none exist.
    pub fn orders_for(&self, user_id: i64) -> Vec<OrderOut> {
        self.orders.get(&user_id).cloned().unwrap_or_default()
    }

    /// Resolves a bearer token to the authenticated user's id.
    ///
    /// # Errors
    ///
    /// Returns `401 Unauthorized` if the token is not recognized.
    pub fn authenticate(&self, token: &str) -> Result<i64> {
        self.tokens
            .get(token)
            .copied()
            .ok_or_else(|| Error::unauthorized("invalid token"))
    }
}
