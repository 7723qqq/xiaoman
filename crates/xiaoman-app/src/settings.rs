//! The settings model and its persistence.
//! and `SettingsStore.kt`: the same keys, defaults, validation and the same recovery from an
//! unreadable secret file.
//!
//! Android keeps everything but the API keys in `SharedPreferences` and the keys alone in
//! `EncryptedSharedPreferences`. Headless, both become one [`KeyValueStore`]: the plain store holds
//! every other value, the shared secret store holds only the API keys. Neither store ever logs or
//! prints a value (see [`JsonFileStore`] and [`InMemoryKeyValueStore`]'s `Debug`), and
//! [`ModelSettings`] masks its key in `Display`.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xiaoman_api::{AgentConfig, ModelConfig, ModelRole};

/// Plain preference file of [`SettingsStore`] inside the settings directory.
pub const PLAIN_FILE: &str = "settings.json";
/// Preference file holding the API keys, shared with [`crate::mcp_settings::McpSettingsStore`].
pub const SECRET_FILE: &str = "settings-secrets.json";

pub const CHAT_PREFIX: &str = "chat.";
pub const OPERATOR_PREFIX: &str = "operator.";
pub const BASE_URL_KEY: &str = "baseUrl";
pub const MODEL_KEY: &str = "model";
pub const API_KEY_KEY: &str = "apiKey";
pub const TEMPERATURE_KEY: &str = "temperature";
pub const MAX_STEPS_KEY: &str = "agent.maxSteps";
pub const LEARN_KEY: &str = "agent.learnFromSuccess";

// ---------------------------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------------------------

/// One preferences file: typed values under string keys, like Android's `SharedPreferences`.
///
/// `Send` so a store can be handed to a background task; mutation needs `&mut self`, which is why
/// [`SharedStore`] wraps it in a lock when two owners need the same file.
/// Builds the settings store of a data directory. Android injects one whose secret half lives in
/// the Keystore (`EncryptedSharedPreferences` in the Kotlin app); everywhere else the default
/// `settings.json` + `settings-secrets.json` pair is used.
pub type SettingsFactory = Arc<dyn Fn(&Path) -> io::Result<SettingsStore> + Send + Sync>;

pub trait KeyValueStore: Send {
    fn get(&self, key: &str) -> Option<Value>;

    fn put(&mut self, key: &str, value: Value) -> io::Result<()>;

    fn remove(&mut self, key: &str) -> io::Result<()>;
}

/// The test double: everything in memory, nothing on disk.
#[derive(Default)]
pub struct InMemoryKeyValueStore {
    values: BTreeMap<String, Value>,
}

impl InMemoryKeyValueStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The stored values, keys sorted. Values are settings and secrets: do not print them.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }
}

impl KeyValueStore for InMemoryKeyValueStore {
    fn get(&self, key: &str) -> Option<Value> {
        self.values.get(key).cloned()
    }

    fn put(&mut self, key: &str, value: Value) -> io::Result<()> {
        self.values.insert(key.to_string(), value);
        Ok(())
    }

    fn remove(&mut self, key: &str) -> io::Result<()> {
        self.values.remove(key);
        Ok(())
    }
}

impl fmt::Debug for InMemoryKeyValueStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryKeyValueStore")
            .field("keys", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Preferences persisted as one JSON object, `{"chat.baseUrl": "...", "agent.maxSteps": 30}`.
///
/// Every `put` and `remove` rewrites the file through a temporary sibling and a rename, so an
/// interrupted write leaves the previous file intact. Keys stay sorted, so the file is stable
/// between runs.
pub struct JsonFileStore {
    path: PathBuf,
    values: BTreeMap<String, Value>,
}

impl JsonFileStore {
    /// Loads `path`. A missing file is an empty store; a file that is not a JSON object is an
    /// error naming the path, because silently starting over would drop the user's settings.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let values = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let value: Value = serde_json::from_str(&text).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{}: not valid settings JSON ({error}); fix or delete the file",
                            path.display()
                        ),
                    )
                })?;
                match value {
                    Value::Object(map) => map.into_iter().collect(),
                    other => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "{}: settings must be a JSON object, found {}; fix or delete the file",
                                path.display(),
                                kind_of(&other)
                            ),
                        ));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("{}: {error}", path.display()),
                ));
            }
        };
        Ok(Self { path, values })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn flush(&self) -> io::Result<()> {
        let text =
            serde_json::to_string_pretty(&Value::Object(self.values.clone().into_iter().collect()))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &self.path)
    }
}

impl KeyValueStore for JsonFileStore {
    fn get(&self, key: &str) -> Option<Value> {
        self.values.get(key).cloned()
    }

    fn put(&mut self, key: &str, value: Value) -> io::Result<()> {
        self.values.insert(key.to_string(), value);
        self.flush()
    }

    fn remove(&mut self, key: &str) -> io::Result<()> {
        self.values.remove(key);
        self.flush()
    }
}

impl fmt::Debug for JsonFileStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonFileStore")
            .field("path", &self.path)
            .field("keys", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// A handle two stores can hold at once, the way [`SettingsStore`] and
/// [`crate::mcp_settings::McpSettingsStore`] share one encrypted preferences file in the app.
///
/// Writes go through one lock, so the two stores cannot overwrite each other's half-loaded view.
#[derive(Clone)]
pub struct SharedStore {
    inner: Arc<Mutex<Box<dyn KeyValueStore>>>,
}

impl SharedStore {
    pub fn new(store: impl KeyValueStore + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(store))),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Box<dyn KeyValueStore>> {
        // A panic while another thread held the store must not take the settings down with it.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl KeyValueStore for SharedStore {
    fn get(&self, key: &str) -> Option<Value> {
        self.lock().get(key)
    }

    fn put(&mut self, key: &str, value: Value) -> io::Result<()> {
        self.lock().put(key, value)
    }

    fn remove(&mut self, key: &str) -> io::Result<()> {
        self.lock().remove(key)
    }
}

impl fmt::Debug for SharedStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SharedStore")
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

// ---------------------------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------------------------

/// One OpenAI-compatible endpoint as typed into Settings. `Display` never shows the API key.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSettings {
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub temperature: Option<f64>,
}

impl ModelSettings {
    /// Volcano Ark, `ModelSettings.DEFAULT_BASE_URL`.
    pub const DEFAULT_BASE_URL: &'static str = "https://ark.cn-beijing.volces.com/api/v3";

    pub fn configured(&self) -> bool {
        has_text(&self.base_url) && has_text(&self.model) && has_text(&self.api_key)
    }

    pub fn to_model_config(&self, role: ModelRole) -> ModelConfig {
        let mut config = ModelConfig::new(
            role,
            self.base_url.trim(),
            self.model.trim(),
            self.api_key.trim(),
        );
        config.temperature = self.temperature;
        config
    }
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            base_url: Self::DEFAULT_BASE_URL.to_string(),
            model: String::new(),
            api_key: String::new(),
            temperature: None,
        }
    }
}

impl fmt::Display for ModelSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ModelSettings(baseUrl={}, model={}, apiKey={}, temperature={})",
            self.base_url,
            self.model,
            masked(&self.api_key),
            temperature_text(self.temperature)
        )
    }
}

impl fmt::Debug for ModelSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Both endpoints plus the agent knobs the Settings screen exposes.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub chat: ModelSettings,
    pub operator: ModelSettings,
    pub max_steps: i32,
    pub learn_from_success: bool,
}

impl AppSettings {
    /// `AppSettings.MAX_STEPS_RANGE`.
    pub const MAX_STEPS_RANGE: RangeInclusive<i32> = 1..=200;

    pub fn configured(&self) -> bool {
        self.chat.configured() && self.operator.configured()
    }

    /// Values to mask in traces and logs.
    pub fn secrets(&self) -> Vec<String> {
        [self.chat.api_key.clone(), self.operator.api_key.clone()]
            .into_iter()
            .filter(|key| has_text(key))
            .collect()
    }

    pub fn agent_config(&self) -> AgentConfig {
        AgentConfig {
            max_steps: self.max_steps,
            learn_from_success: self.learn_from_success,
            ..AgentConfig::default()
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        let agent = AgentConfig::default();
        Self {
            chat: ModelSettings::default(),
            operator: ModelSettings::default(),
            max_steps: agent.max_steps,
            learn_from_success: agent.learn_from_success,
        }
    }
}

impl fmt::Display for AppSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AppSettings(chat={}, operator={}, maxSteps={}, learnFromSuccess={})",
            self.chat, self.operator, self.max_steps, self.learn_from_success
        )
    }
}

impl fmt::Debug for AppSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The editable text of the Settings screen. Parsing lives here, not in a screen.
#[derive(Clone, PartialEq, Eq)]
pub struct SettingsForm {
    pub chat: ModelForm,
    pub operator: ModelForm,
    pub max_steps: String,
    pub learn_from_success: bool,
}

/// One endpoint as typed: the temperature is still text, so half-typed numbers survive.
#[derive(Clone, PartialEq, Eq)]
pub struct ModelForm {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub temperature: String,
}

impl ModelForm {
    pub fn temperature_valid(&self) -> bool {
        SettingsForm::parse_temperature(&self.temperature).is_some()
            || self.temperature.trim().is_empty()
    }

    /// `None` while the temperature is not a number in `0.0..=2.0`.
    pub fn to_settings(&self) -> Option<ModelSettings> {
        let temperature = if self.temperature.trim().is_empty() {
            None
        } else {
            Some(SettingsForm::parse_temperature(&self.temperature)?)
        };
        Some(ModelSettings {
            base_url: self.base_url.trim().to_string(),
            model: self.model.trim().to_string(),
            api_key: self.api_key.trim().to_string(),
            temperature,
        })
    }

    pub fn of(settings: &ModelSettings) -> Self {
        Self {
            base_url: settings.base_url.clone(),
            model: settings.model.clone(),
            api_key: settings.api_key.clone(),
            temperature: settings.temperature.map(double_text).unwrap_or_default(),
        }
    }
}

impl Default for ModelForm {
    fn default() -> Self {
        Self {
            base_url: ModelSettings::DEFAULT_BASE_URL.to_string(),
            model: String::new(),
            api_key: String::new(),
            temperature: String::new(),
        }
    }
}

impl fmt::Display for ModelForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ModelForm(baseUrl={}, model={}, apiKey={}, temperature={})",
            self.base_url,
            self.model,
            masked(&self.api_key),
            self.temperature
        )
    }
}

impl fmt::Debug for ModelForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl SettingsForm {
    pub fn max_steps_valid(&self) -> bool {
        self.max_steps
            .trim()
            .parse::<i32>()
            .ok()
            .is_some_and(|steps| AppSettings::MAX_STEPS_RANGE.contains(&steps))
    }

    /// `None` while a field is invalid.
    pub fn to_settings(&self) -> Option<AppSettings> {
        let steps = self
            .max_steps
            .trim()
            .parse::<i32>()
            .ok()
            .filter(|steps| AppSettings::MAX_STEPS_RANGE.contains(steps))?;
        Some(AppSettings {
            chat: self.chat.to_settings()?,
            operator: self.operator.to_settings()?,
            max_steps: steps,
            learn_from_success: self.learn_from_success,
        })
    }

    pub fn of(settings: &AppSettings) -> Self {
        Self {
            chat: ModelForm::of(&settings.chat),
            operator: ModelForm::of(&settings.operator),
            max_steps: settings.max_steps.to_string(),
            learn_from_success: settings.learn_from_success,
        }
    }

    /// A number in `0.0..=2.0`, else `None`; `NaN` and infinities are rejected.
    pub fn parse_temperature(text: &str) -> Option<f64> {
        text.trim()
            .parse::<f64>()
            .ok()
            .filter(|value| (0.0..=2.0).contains(value))
    }
}

impl Default for SettingsForm {
    fn default() -> Self {
        let settings = AppSettings::default();
        Self {
            chat: ModelForm::default(),
            operator: ModelForm::default(),
            max_steps: settings.max_steps.to_string(),
            learn_from_success: settings.learn_from_success,
        }
    }
}

impl fmt::Display for SettingsForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SettingsForm(chat={}, operator={}, maxSteps={}, learnFromSuccess={})",
            self.chat, self.operator, self.max_steps, self.learn_from_success
        )
    }
}

impl fmt::Debug for SettingsForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ---------------------------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------------------------

/// Persists [`AppSettings`]: everything but the API keys in `plain`, the keys only in `secret`.
///
/// The two files are separate writes, in the order the app writes them; the in-memory value only
/// changes once both succeeded, so a failed write leaves the screen showing what is stored.
pub struct SettingsStore {
    plain: Box<dyn KeyValueStore>,
    secret: SharedStore,
    settings: AppSettings,
}

impl SettingsStore {
    /// Loads what the stores hold now, like the Kotlin store's constructor.
    pub fn new(plain: Box<dyn KeyValueStore>, secret: SharedStore) -> Self {
        let settings = Self::load(plain.as_ref(), &secret);
        Self {
            plain,
            secret,
            settings,
        }
    }

    /// `settings.json` and `settings-secrets.json` inside `dir` — the injected replacement for
    /// Android's `Context`-bound preference files.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        Self::open_with(dir, |_| {})
    }

    /// [`SettingsStore::open`], with `warn` hearing about a secret file that had to be reset.
    pub fn open_with(dir: impl AsRef<Path>, warn: impl Fn(&str)) -> io::Result<Self> {
        let dir = dir.as_ref();
        let plain: Box<dyn KeyValueStore> = Box::new(JsonFileStore::open(dir.join(PLAIN_FILE))?);
        let secret = match JsonFileStore::open(dir.join(SECRET_FILE)) {
            Ok(store) => store,
            Err(error) => {
                // Kotlin: a keystore entry lost after a restore or an OS update makes the file
                // undecryptable. The keys cannot be recovered; start over so the user can type
                // them again.
                let path = dir.join(SECRET_FILE);
                warn(&format!(
                    "settings: {} unreadable ({error}); resetting API keys",
                    path.display()
                ));
                remove_if_present(&path)?;
                JsonFileStore::open(&path)?
            }
        };
        Ok(Self::new(plain, SharedStore::new(secret)))
    }

    pub fn current(&self) -> &AppSettings {
        &self.settings
    }

    /// The secret file, for [`crate::mcp_settings::McpSettingsStore`] to share.
    pub fn secret(&self) -> SharedStore {
        self.secret.clone()
    }

    pub fn save(&mut self, settings: AppSettings) -> io::Result<()> {
        for (prefix, model) in [
            (CHAT_PREFIX, &settings.chat),
            (OPERATOR_PREFIX, &settings.operator),
        ] {
            self.plain.put(
                &format!("{prefix}{BASE_URL_KEY}"),
                Value::from(model.base_url.clone()),
            )?;
            self.plain.put(
                &format!("{prefix}{MODEL_KEY}"),
                Value::from(model.model.clone()),
            )?;
            match model.temperature {
                Some(temperature) => {
                    self.plain.put(
                        &format!("{prefix}{TEMPERATURE_KEY}"),
                        Value::from(temperature),
                    )?;
                }
                None => self.plain.remove(&format!("{prefix}{TEMPERATURE_KEY}"))?,
            }
        }
        self.plain
            .put(MAX_STEPS_KEY, Value::from(settings.max_steps))?;
        self.plain
            .put(LEARN_KEY, Value::from(settings.learn_from_success))?;
        self.secret.put(
            &format!("{CHAT_PREFIX}{API_KEY_KEY}"),
            Value::from(settings.chat.api_key.clone()),
        )?;
        self.secret.put(
            &format!("{OPERATOR_PREFIX}{API_KEY_KEY}"),
            Value::from(settings.operator.api_key.clone()),
        )?;
        self.settings = settings;
        Ok(())
    }

    fn load(plain: &dyn KeyValueStore, secret: &dyn KeyValueStore) -> AppSettings {
        let defaults = AppSettings::default();
        AppSettings {
            chat: model(plain, secret, CHAT_PREFIX, &defaults.chat),
            operator: model(plain, secret, OPERATOR_PREFIX, &defaults.operator),
            max_steps: plain
                .get(MAX_STEPS_KEY)
                .and_then(|value| value.as_i64())
                .and_then(|steps| i32::try_from(steps).ok())
                .unwrap_or(defaults.max_steps),
            learn_from_success: plain
                .get(LEARN_KEY)
                .and_then(|value| value.as_bool())
                .unwrap_or(defaults.learn_from_success),
        }
    }
}

impl fmt::Display for SettingsStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SettingsStore(settings={})", self.settings)
    }
}

impl fmt::Debug for SettingsStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

fn model(
    plain: &dyn KeyValueStore,
    secret: &dyn KeyValueStore,
    prefix: &str,
    defaults: &ModelSettings,
) -> ModelSettings {
    let text = |store: &dyn KeyValueStore, suffix: &str| {
        store
            .get(&format!("{prefix}{suffix}"))
            .and_then(|value| value.as_str().map(str::to_string))
    };
    ModelSettings {
        base_url: text(plain, BASE_URL_KEY).unwrap_or_else(|| defaults.base_url.clone()),
        model: text(plain, MODEL_KEY).unwrap_or_default(),
        api_key: text(secret, API_KEY_KEY).unwrap_or_default(),
        temperature: temperature(plain, &format!("{prefix}{TEMPERATURE_KEY}")),
    }
}

/// Kotlin stores the temperature as text; a hand-edited number loads too.
fn temperature(store: &dyn KeyValueStore, key: &str) -> Option<f64> {
    match store.get(key)? {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn has_text(value: &str) -> bool {
    !value.trim().is_empty()
}

fn masked(api_key: &str) -> &'static str {
    if api_key.is_empty() { "<unset>" } else { "***" }
}

/// Kotlin prints `null` for an absent `Double?`; Rust's `Option` renders `None`.
fn temperature_text(temperature: Option<f64>) -> String {
    match temperature {
        Some(value) => double_text(value),
        None => "null".to_string(),
    }
}

/// `Double.toString()` keeps the decimal point, so a temperature of 1 is written `1.0`.
pub fn double_text(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}
