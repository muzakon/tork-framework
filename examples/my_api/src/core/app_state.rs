//! Application resources and lifespan.
//!
//! `AppState` is only a resource container produced by the lifespan. Its
//! resources (the store and the config) are registered by type and injected
//! directly into services and handlers, so no code reaches through a state
//! object.

use std::collections::HashMap;
use std::sync::Arc;

use tork::{Error, Hub, LifespanContext, Resources, Result, SecretString, settings};

use crate::models::chat::ChatMessage;
use crate::models::order::OrderOut;
use crate::models::user::UserOut;

/// Broadcast hub for the chat demo, injected as a resource.
///
/// A local newtype around [`Hub`] so a `FromRequest` can be generated for it
/// (the orphan rule forbids implementing it for the foreign `Hub` directly).
#[derive(Clone)]
pub struct ChatHub(pub Hub<ChatMessage>);

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

/// Network settings, nested inside [`Config`].
#[settings]
pub struct ServerConfig {
    /// Address the server binds to.
    #[setting(default = "0.0.0.0")]
    pub host: String,
    /// Port the server listens on.
    #[setting(default = 8000)]
    pub port: u16,
}

/// Application configuration, loaded once at startup and injected as a resource.
///
/// Values come from `config/default.toml`, then `config/{env}.toml`, then `.env`
/// and environment variables (prefix `APP_`, nesting via `__`). Defaults fill in
/// anything missing, and the whole config is validated before the app runs.
#[settings(
    prefix = "APP",
    env_file = ".env",
    config_file = "config/default.toml",
    files = ["config/{env}.toml"]
)]
pub struct Config {
    /// Human-readable application name.
    #[setting(default = "my_api")]
    pub app_name: String,
    /// Deployment environment (also selects `config/{env}.toml`).
    #[setting(default = "development")]
    pub environment: String,
    /// Maximum items returned per user.
    #[setting(default = 50, ge = 1, le = 500)]
    pub items_per_user: u32,
    /// Network settings. Defaults to `ServerConfig`'s own defaults when absent.
    #[setting(nested, default)]
    pub server: ServerConfig,
    /// A sample secret; override it through the environment in production.
    #[setting(secret, default = "dev-placeholder-key")]
    pub api_key: SecretString,
}

/// The application's resource container.
#[derive(Clone, Resources)]
pub struct AppState {
    #[resource]
    pub store: UserStore,
    #[resource]
    pub config: Arc<Config>,
    #[resource]
    pub chat: ChatHub,
}

#[tork::lifespan]
impl AppState {
    /// Acquires the application's resources at startup.
    async fn startup(_ctx: LifespanContext) -> Result<Self> {
        // Load and validate configuration once; a failure aborts the boot.
        let config = Arc::new(Config::load()?);
        Ok(AppState {
            store: UserStore::seed(),
            config,
            chat: ChatHub(Hub::new()),
        })
    }

    /// Releases the application's resources at shutdown.
    async fn shutdown(self) -> Result<()> {
        eprintln!("my_api: application stopped");
        Ok(())
    }
}
