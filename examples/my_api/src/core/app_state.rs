//! Application resources and lifespan.
//!
//! `AppState` is only a resource container produced by the lifespan. Its
//! resources (the store and the config) are registered by type and injected
//! directly into services and handlers, so no code reaches through a state
//! object.

use std::collections::HashMap;
use std::sync::Arc;

use tork::{Error, LifespanContext, Resources, Result};

use crate::models::order::OrderOut;
use crate::models::user::UserOut;

/// In-memory data store, injected as a resource.
///
/// Held behind `Arc`s so cloning (which happens on each injection) is cheap. In
/// a real service this would wrap connection pools.
#[derive(Clone)]
pub struct UserStore {
    users: Arc<HashMap<i64, UserOut>>,
    orders: Arc<HashMap<i64, Vec<OrderOut>>>,
    tokens: Arc<HashMap<String, i64>>,
}

impl UserStore {
    /// Builds the store with a seeded in-memory data set.
    pub fn seed() -> Self {
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

        let tokens = HashMap::from([("ada-token".to_owned(), 1), ("alan-token".to_owned(), 2)]);

        Self {
            users: Arc::new(users),
            orders: Arc::new(orders),
            tokens: Arc::new(tokens),
        }
    }

    /// Looks up a user by id.
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
    pub fn authenticate(&self, token: &str) -> Result<i64> {
        self.tokens
            .get(token)
            .copied()
            .ok_or_else(|| Error::unauthorized("invalid token"))
    }
}

/// Application configuration, injected as a resource.
#[derive(Clone)]
pub struct Config {
    pub service_name: String,
}

/// The application's resource container.
#[derive(Clone, Resources)]
pub struct AppState {
    #[resource]
    pub store: UserStore,
    #[resource]
    pub config: Config,
}

#[tork::lifespan]
impl AppState {
    /// Acquires the application's resources at startup.
    async fn startup(ctx: LifespanContext) -> Result<Self> {
        let service_name = ctx.env("SERVICE_NAME").unwrap_or_else(|_| "my_api".to_owned());
        Ok(AppState {
            store: UserStore::seed(),
            config: Config { service_name },
        })
    }

    /// Releases the application's resources at shutdown.
    async fn shutdown(self) -> Result<()> {
        eprintln!("my_api: application stopped");
        Ok(())
    }
}
