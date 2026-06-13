//! Typed application configuration.
//!
//! [`SettingsLoader`] merges configuration from several sources into one typed,
//! validated value, loaded once at startup. The sources, from lowest to highest
//! precedence, are: struct defaults, a base config file, environment-specific
//! files, a `.env` file, environment variables, a secrets directory, and explicit
//! overrides. [`SecretString`] holds sensitive values without exposing them in
//! logs.
//!
//! The `#[settings]` macro generates a struct that derives `Deserialize` and
//! `garde::Validate` and a `load()` method built on this loader, but the loader is
//! usable directly with any `DeserializeOwned + garde::Validate` type.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use garde::Validate;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::error::{Error, Result};

/// Placeholder rendered as a masked secret value.
const REDACTED: &str = "********";
/// Default environment name when none is set.
const DEFAULT_ENVIRONMENT: &str = "development";
/// Separator marking a nesting level in an environment variable or secret name.
const NESTING_SEPARATOR: &str = "__";
/// Token replaced with the resolved environment name in a file path.
const ENV_PLACEHOLDER: &str = "{env}";

/// A string whose value is kept out of logs and debug output.
///
/// `Debug` and `Display` render a fixed mask; the value is only readable through
/// [`SecretString::expose`]. It deserializes from a plain string, so it can stand
/// in for any secret configuration field. It is intentionally not `Serialize`, so
/// a configuration carrying secrets cannot be written back out by accident.
#[derive(Clone, serde::Deserialize)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a value as a secret.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the underlying value. This is the only way to read a secret, so
    /// call sites that expose it are easy to audit.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SecretString").field(&REDACTED).finish()
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Loads and validates a typed configuration from layered sources.
///
/// Build it with the source methods, then call [`SettingsLoader::load`]. Each
/// source overrides the ones before it; see the module documentation for the full
/// precedence order.
pub struct SettingsLoader<T> {
    env_file: Option<PathBuf>,
    prefix: Option<String>,
    config_file: Option<String>,
    files: Vec<String>,
    secrets_dir: Option<PathBuf>,
    overrides: Map<String, Value>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Default for SettingsLoader<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> SettingsLoader<T> {
    /// Creates a loader with no sources configured.
    pub fn new() -> Self {
        Self {
            env_file: None,
            prefix: None,
            config_file: None,
            files: Vec::new(),
            secrets_dir: None,
            overrides: Map::new(),
            _marker: PhantomData,
        }
    }

    /// Loads variables from this `.env` file before reading the environment.
    /// Existing environment variables are not overwritten.
    pub fn env_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.env_file = Some(path.into());
        self
    }

    /// Reads environment variables that start with `prefix` followed by `_`.
    /// Nested fields use `__` between levels (for example `APP_SERVER__PORT`).
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Reads a base configuration file (TOML). A missing file is ignored.
    pub fn config_file(mut self, path: impl Into<String>) -> Self {
        self.config_file = Some(path.into());
        self
    }

    /// Appends an environment-specific file (TOML). The `{env}` token in the path
    /// is replaced with the resolved environment name. A missing file is ignored.
    pub fn file(mut self, path: impl Into<String>) -> Self {
        self.files.push(path.into());
        self
    }

    /// Reads one secret per file from a directory: the file name is the key (with
    /// `__` marking nesting) and the trimmed contents are the value.
    pub fn secrets_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.secrets_dir = Some(dir.into());
        self
    }

    /// Sets the highest-priority override for a key, overriding every source.
    /// The key uses `.` between nesting levels (for example `server.port`).
    pub fn override_value(mut self, key: impl AsRef<str>, value: impl serde::Serialize) -> Self {
        if let Ok(value) = serde_json::to_value(value) {
            let parts: Vec<&str> = key.as_ref().split('.').collect();
            insert_nested(&mut self.overrides, &parts, value);
        }
        self
    }

    /// Resolves the current environment name from `{PREFIX}_ENV` (or `ENV` when no
    /// prefix is set), defaulting to `development`.
    fn environment_name(&self) -> String {
        let var = match &self.prefix {
            Some(prefix) => format!("{prefix}_ENV"),
            None => "ENV".to_owned(),
        };
        std::env::var(&var).unwrap_or_else(|_| DEFAULT_ENVIRONMENT.to_owned())
    }

    /// Builds the environment-variable provider with the configured prefix.
    fn env_provider(&self) -> Env {
        match &self.prefix {
            Some(prefix) => Env::prefixed(&format!("{prefix}_")).split(NESTING_SEPARATOR),
            None => Env::raw().split(NESTING_SEPARATOR),
        }
    }
}

impl<T: DeserializeOwned + Validate<Context = ()>> SettingsLoader<T> {
    /// Merges every source, deserializes into `T`, and validates it.
    ///
    /// Returns an error with code `CONFIG_LOAD_FAILED` when a source cannot be
    /// parsed into `T`, or `CONFIG_VALIDATION_ERROR` (with field details) when the
    /// value fails validation. Used at startup, either aborts before the app runs.
    pub fn load(self) -> Result<T> {
        // `dotenvy` writes into the process environment via `set_var`, which races
        // with any concurrent environment read/write from another thread. Hold the
        // shared environment lock across the `.env` load and the environment reads
        // (env name + the figment env provider) so concurrent `load()` calls
        // serialize instead of racing. Mutating `std::env` from outside the loader
        // while it runs is still the caller's responsibility to avoid.
        let _env_guard = crate::env::env_guard();
        self.load_locked()
    }

    /// Loads the configuration assuming the caller already holds the shared
    /// environment lock (see [`load`](SettingsLoader::load)).
    ///
    /// Kept separate so callers that must hold the lock across their own
    /// environment mutations (the tests below) do not deadlock by re-locking a
    /// non-reentrant mutex inside `load`.
    fn load_locked(self) -> Result<T> {
        // Load the `.env` file into the process environment. dotenvy does not
        // overwrite existing variables, so a real environment variable still wins.
        match &self.env_file {
            Some(path) => {
                let _ = dotenvy::from_path(path);
            }
            None => {
                let _ = dotenvy::dotenv();
            }
        }

        let environment = self.environment_name();

        let mut figment = Figment::new();
        if let Some(config_file) = &self.config_file {
            figment = figment.merge(Toml::file(config_file));
        }
        for file in &self.files {
            let resolved = file.replace(ENV_PLACEHOLDER, &environment);
            figment = figment.merge(Toml::file(resolved));
        }
        figment = figment.merge(self.env_provider());
        if let Some(dir) = &self.secrets_dir {
            let secrets = read_secrets(dir);
            if !secrets.is_empty() {
                figment = figment.merge(Serialized::defaults(Value::Object(secrets)));
            }
        }
        if !self.overrides.is_empty() {
            figment = figment.merge(Serialized::defaults(Value::Object(self.overrides.clone())));
        }

        let value: T = figment.extract().map_err(|error| {
            let message = error.to_string();
            Error::internal(format!("failed to load configuration: {message}"))
                .with_code("CONFIG_LOAD_FAILED")
                .with_source(error)
        })?;

        value.validate().map_err(|report| {
            Error::from_garde_report(report).with_code("CONFIG_VALIDATION_ERROR")
        })?;

        Ok(value)
    }
}

/// Reads a secrets directory into a nested map keyed by file name.
fn read_secrets(dir: &Path) -> Map<String, Value> {
    let mut root = Map::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return root;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let key = name.to_lowercase();
        let parts: Vec<&str> = key.split(NESTING_SEPARATOR).collect();
        insert_nested(&mut root, &parts, Value::String(contents.trim().to_owned()));
    }
    root
}

/// Inserts `value` into `root` along a path of nested object keys.
fn insert_nested(root: &mut Map<String, Value>, parts: &[&str], value: Value) {
    match parts {
        [] => {}
        [key] => {
            root.insert((*key).to_owned(), value);
        }
        [key, rest @ ..] => {
            let entry = root
                .entry((*key).to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            if let Value::Object(map) = entry {
                insert_nested(map, rest, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::env_guard;
    use garde::Validate;
    use serde::Deserialize;
    use std::io::Write;

    #[derive(Debug, Deserialize, Validate)]
    struct Nested {
        #[garde(skip)]
        host: String,
        #[garde(range(min = 1, max = 65535))]
        port: u16,
    }

    #[derive(Debug, Deserialize, Validate)]
    struct Sample {
        #[serde(default = "default_name")]
        #[garde(skip)]
        name: String,
        #[garde(range(min = 1, max = 500))]
        items: u32,
        #[garde(dive)]
        nested: Nested,
        #[garde(skip)]
        token: SecretString,
    }

    fn default_name() -> String {
        "Awesome API".to_owned()
    }

    #[test]
    fn builder_methods_store_configuration_sources() {
        let loader = SettingsLoader::<Sample>::default()
            .env_file("config/.env.test")
            .prefix("CFGTESTZ")
            .config_file("config/base.toml")
            .file("config/{env}.toml")
            .secrets_dir("secrets")
            .override_value("nested.port", 7000u16);

        assert_eq!(
            loader.env_file.as_deref(),
            Some(Path::new("config/.env.test"))
        );
        assert_eq!(loader.prefix.as_deref(), Some("CFGTESTZ"));
        assert_eq!(loader.config_file.as_deref(), Some("config/base.toml"));
        assert_eq!(loader.files, vec!["config/{env}.toml"]);
        assert_eq!(loader.secrets_dir.as_deref(), Some(Path::new("secrets")));
        assert_eq!(loader.overrides["nested"]["port"], Value::from(7000u16));
    }

    #[test]
    fn environment_name_uses_prefix_and_default() {
        let _guard = env_guard();
        std::env::remove_var("ENV");
        std::env::remove_var("CFGTESTENV_ENV");
        assert_eq!(
            SettingsLoader::<Sample>::new().environment_name(),
            "development"
        );

        std::env::set_var("ENV", "staging");
        std::env::set_var("CFGTESTENV_ENV", "production");
        assert_eq!(
            SettingsLoader::<Sample>::new().environment_name(),
            "staging"
        );
        assert_eq!(
            SettingsLoader::<Sample>::new()
                .prefix("CFGTESTENV")
                .environment_name(),
            "production"
        );

        std::env::remove_var("ENV");
        std::env::remove_var("CFGTESTENV_ENV");
    }

    #[test]
    fn read_secrets_and_insert_nested_cover_edge_cases() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("TOKEN"), "  shh \n").unwrap();
        std::fs::write(dir.path().join("DB__PORT"), "5432").unwrap();

        let secrets = read_secrets(dir.path());
        assert_eq!(secrets["token"], "shh");
        assert_eq!(secrets["db"]["port"], "5432");
        assert!(read_secrets(&dir.path().join("missing")).is_empty());

        let mut root = Map::new();
        insert_nested(&mut root, &[], Value::from("ignored"));
        assert!(root.is_empty());

        root.insert("db".to_owned(), Value::from("scalar"));
        insert_nested(&mut root, &["db", "host"], Value::from("localhost"));
        assert_eq!(root["db"]["host"], "localhost");
    }

    #[test]
    fn secret_string_is_masked_but_exposable() {
        let secret = SecretString::new("super-secret");
        assert_eq!(format!("{secret:?}"), "SecretString(\"********\")");
        assert_eq!(format!("{secret}"), "********");
        assert_eq!(secret.expose(), "super-secret");
    }

    #[test]
    fn defaults_apply_and_overrides_win() {
        // A unique prefix keeps this test independent of other tests' env vars.
        let value: Sample = SettingsLoader::new()
            .prefix("CFGTESTA")
            .override_value("items", 42u32)
            .override_value("nested.host", "localhost")
            .override_value("nested.port", 8080u16)
            .override_value("token", "shh")
            .load()
            .expect("load should succeed");

        assert_eq!(value.name, "Awesome API"); // serde default
        assert_eq!(value.items, 42);
        assert_eq!(value.nested.host, "localhost");
        assert_eq!(value.nested.port, 8080);
        assert_eq!(value.token.expose(), "shh");
    }

    #[test]
    fn environment_variable_overrides_and_nests() {
        let _guard = env_guard();
        // The prefix is unique to this test, so the variables do not collide.
        std::env::set_var("CFGTESTB_NAME", "From Env");
        std::env::set_var("CFGTESTB_ITEMS", "7");
        std::env::set_var("CFGTESTB_NESTED__HOST", "db.internal");
        std::env::set_var("CFGTESTB_NESTED__PORT", "5432");
        std::env::set_var("CFGTESTB_TOKEN", "envtoken");

        let value: Sample = SettingsLoader::new()
            .prefix("CFGTESTB")
            // The guard is already held above, so use the non-locking variant.
            .load_locked()
            .expect("load should succeed");

        assert_eq!(value.name, "From Env");
        assert_eq!(value.items, 7);
        assert_eq!(value.nested.host, "db.internal");
        assert_eq!(value.nested.port, 5432);
        assert_eq!(value.token.expose(), "envtoken");

        for key in ["NAME", "ITEMS", "NESTED__HOST", "NESTED__PORT", "TOKEN"] {
            std::env::remove_var(format!("CFGTESTB_{key}"));
        }
    }

    #[test]
    fn environment_variable_overrides_a_config_file() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(
            file,
            "name = \"From File\"\nitems = 3\ntoken = \"filetoken\"\n\n[nested]\nhost = \"file.host\"\nport = 1111"
        )
        .unwrap();

        // The environment value must win over the file value.
        std::env::set_var("CFGTESTD_ITEMS", "9");

        let value: Sample = SettingsLoader::new()
            .prefix("CFGTESTD")
            .config_file(path.to_string_lossy().into_owned())
            // The guard is already held above, so use the non-locking variant.
            .load_locked()
            .expect("load should succeed");

        assert_eq!(value.name, "From File"); // only in the file
        assert_eq!(value.items, 9); // env overrides the file
        assert_eq!(value.nested.host, "file.host");
        assert_eq!(value.nested.port, 1111);

        std::env::remove_var("CFGTESTD_ITEMS");
    }

    #[test]
    fn validation_failure_is_reported() {
        let error = SettingsLoader::<Sample>::new()
            .prefix("CFGTESTC")
            .override_value("items", 9999u32) // exceeds max of 500
            .override_value("nested.host", "localhost")
            .override_value("nested.port", 80u16)
            .override_value("token", "shh")
            .load()
            .unwrap_err();

        assert_eq!(error.code(), "CONFIG_VALIDATION_ERROR");
    }

    #[test]
    fn load_merges_env_file_environment_specific_file_and_secrets() {
        let _guard = env_guard();
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.toml");
        let env_file = dir.path().join(".env");
        let env_toml = dir.path().join("production.toml");
        let secrets = dir.path().join("secrets");
        std::fs::create_dir(&secrets).unwrap();

        let mut base_file = std::fs::File::create(&base).unwrap();
        writeln!(
            base_file,
            "name = \"Base\"\nitems = 3\ntoken = \"from-base\"\n\n[nested]\nhost = \"file.host\"\nport = 1111"
        )
        .unwrap();
        std::fs::write(&env_file, "CFGTESTE_ENV=production\nCFGTESTE_ITEMS=9\n").unwrap();
        let mut env_override = std::fs::File::create(&env_toml).unwrap();
        writeln!(
            env_override,
            "name = \"Prod\"\n[nested]\nhost = \"prod.host\""
        )
        .unwrap();
        std::fs::write(secrets.join("TOKEN"), "from-secret\n").unwrap();

        let value: Sample = SettingsLoader::new()
            .env_file(&env_file)
            .prefix("CFGTESTE")
            .config_file(base.to_string_lossy().into_owned())
            .file(dir.path().join("{env}.toml").to_string_lossy().into_owned())
            .secrets_dir(&secrets)
            // The guard is already held above, so use the non-locking variant.
            .load_locked()
            .expect("load should succeed");

        assert_eq!(value.name, "Prod");
        assert_eq!(value.items, 9);
        assert_eq!(value.nested.host, "prod.host");
        assert_eq!(value.nested.port, 1111);
        assert_eq!(value.token.expose(), "from-secret");

        std::env::remove_var("CFGTESTE_ENV");
        std::env::remove_var("CFGTESTE_ITEMS");
    }

    #[test]
    fn load_reports_configuration_parse_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.toml");
        std::fs::write(
            &path,
            "items = \"oops\"\ntoken = \"x\"\n[nested]\nhost = \"h\"\nport = 1",
        )
        .unwrap();

        let error = SettingsLoader::<Sample>::new()
            .config_file(path.to_string_lossy().into_owned())
            .load()
            .unwrap_err();

        assert_eq!(error.code(), "CONFIG_LOAD_FAILED");
        assert!(error.message().starts_with("failed to load configuration:"));
    }
}
