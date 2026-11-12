//! The Keystore-backed secret half of the Android settings store.
//!
//! The Kotlin app keeps everything but the API keys in `SharedPreferences` and the keys alone in
//! `EncryptedSharedPreferences`. This keeps the same split: the plain half stays the
//! `settings.json` file, and every secret value is one AES-GCM entry that Kotlin's `SecretStore`
//! encrypts with an Android Keystore key. Rust sees one JSON value per key and never writes the
//! keys to the filesystem.

use std::io;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use xiaoman_app::settings::{
    JsonFileStore, KeyValueStore, SettingsFactory, SettingsStore, SharedStore,
};

use crate::session::{read_secret, remove_secret, write_secret};

/// One value per key, encrypted and stored by `dev.xiaoman.app.SecretStore`.
struct KeystoreStore;

impl KeyValueStore for KeystoreStore {
    fn get(&self, key: &str) -> Option<Value> {
        read_secret(key).and_then(|text| serde_json::from_str(&text).ok())
    }

    fn put(&mut self, key: &str, value: Value) -> io::Result<()> {
        let text = serde_json::to_string(&value).map_err(io::Error::other)?;
        if write_secret(key, &text) {
            Ok(())
        } else {
            Err(io::Error::other(
                "the Keystore secret store refused the write",
            ))
        }
    }

    fn remove(&mut self, key: &str) -> io::Result<()> {
        remove_secret(key);
        Ok(())
    }
}

/// `settings.json` under `dir`, and API keys through the Kotlin `SecretStore`.
pub fn factory() -> SettingsFactory {
    Arc::new(|dir: &Path| {
        let plain: Box<dyn KeyValueStore> =
            Box::new(JsonFileStore::open(dir.join("settings.json"))?);
        Ok(SettingsStore::new(plain, SharedStore::new(KeystoreStore)))
    })
}
