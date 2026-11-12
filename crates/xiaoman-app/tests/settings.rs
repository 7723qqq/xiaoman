//! Ports of `app/src/test/kotlin/dev/xiaoman/app/settings/SettingsTest.kt`, plus the file store
//! and the recovery from an unreadable secret file.

use std::sync::{Arc, Mutex};

use tempfile::TempDir;
use xiaoman_api::ModelRole;
use xiaoman_app::settings::{
    AppSettings, InMemoryKeyValueStore, LEARN_KEY, MAX_STEPS_KEY, ModelForm, ModelSettings,
    PLAIN_FILE, SECRET_FILE, SettingsForm, SettingsStore, SharedStore,
};

const KEY: &str = "ark-0123456789abcdef";

fn sample() -> AppSettings {
    AppSettings {
        chat: ModelSettings {
            model: "doubao-chat".to_string(),
            api_key: KEY.to_string(),
            temperature: Some(0.3),
            ..ModelSettings::default()
        },
        operator: ModelSettings {
            base_url: "https://example.com/v1".to_string(),
            model: "doubao-vision".to_string(),
            api_key: KEY.to_string(),
            temperature: None,
        },
        max_steps: 12,
        learn_from_success: false,
    }
}

fn in_memory_store() -> SettingsStore {
    SettingsStore::new(
        Box::new(SharedStore::new(InMemoryKeyValueStore::new())),
        SharedStore::new(InMemoryKeyValueStore::new()),
    )
}

fn file_store(dir: &TempDir) -> SettingsStore {
    SettingsStore::open(dir.path()).expect("settings directory")
}

#[test]
fn defaults_point_at_volcano_ark_and_are_not_configured() {
    let settings = in_memory_store().current().clone();
    assert_eq!(
        settings.chat.base_url,
        "https://ark.cn-beijing.volces.com/api/v3"
    );
    assert!(!settings.configured());
    assert_eq!(settings.max_steps, 30);
    assert!(settings.learn_from_success);
}

#[test]
fn round_trip_keeps_every_field_and_keys_stay_out_of_the_plain_file() {
    let dir = TempDir::new().unwrap();
    file_store(&dir).save(sample()).unwrap();
    let loaded = file_store(&dir).current().clone();
    assert_eq!(loaded, sample());
    assert!(loaded.configured());

    let plain = std::fs::read_to_string(dir.path().join(PLAIN_FILE)).unwrap();
    assert!(!plain.contains(KEY), "{plain}");
    assert!(plain.contains("chat.baseUrl"), "{plain}");
    assert!(plain.contains(MAX_STEPS_KEY), "{plain}");
    assert!(plain.contains(LEARN_KEY), "{plain}");

    let secret = std::fs::read_to_string(dir.path().join(SECRET_FILE)).unwrap();
    assert!(secret.contains(KEY), "{secret}");
}

#[test]
fn to_string_never_shows_the_api_key() {
    let settings = sample();
    assert!(!settings.to_string().contains(KEY));
    assert!(!format!("{settings:?}").contains(KEY));
    assert!(!settings.chat.to_string().contains(KEY));
    assert!(!format!("{:?}", settings.chat).contains(KEY));
    let config = settings.chat.to_model_config(ModelRole::Chat);
    assert!(!format!("{config:?}").contains(KEY));
    assert!(!SettingsForm::of(&settings).to_string().contains(KEY));
    assert!(!format!("{:?}", SettingsForm::of(&settings)).contains(KEY));
    assert!(settings.chat.to_string().contains("***"));
    assert!(settings.chat.to_string().contains("temperature=0.3"));
    assert!(settings.operator.to_string().contains("temperature=null"));
}

#[test]
fn form_parses_and_validates_numbers() {
    let form = SettingsForm::of(&sample());
    assert_eq!(form.to_settings(), Some(sample()));
    assert!(
        SettingsForm {
            max_steps: "0".to_string(),
            ..form.clone()
        }
        .to_settings()
        .is_none()
    );
    assert!(
        SettingsForm {
            chat: ModelForm {
                temperature: "hot".to_string(),
                ..form.chat.clone()
            },
            ..form.clone()
        }
        .to_settings()
        .is_none()
    );
    let blank = SettingsForm {
        chat: ModelForm {
            temperature: String::new(),
            ..form.chat.clone()
        },
        ..form.clone()
    }
    .to_settings();
    assert_eq!(blank.and_then(|settings| settings.chat.temperature), None);
}

#[test]
fn form_accepts_only_temperatures_in_range() {
    assert_eq!(SettingsForm::parse_temperature("0"), Some(0.0));
    assert_eq!(SettingsForm::parse_temperature(" 2 "), Some(2.0));
    assert_eq!(SettingsForm::parse_temperature("2.01"), None);
    assert_eq!(SettingsForm::parse_temperature("-0.1"), None);
    assert_eq!(SettingsForm::parse_temperature("NaN"), None);
    assert_eq!(SettingsForm::parse_temperature(""), None);
    assert!(
        ModelForm {
            temperature: "1.5".to_string(),
            ..ModelForm::default()
        }
        .temperature_valid()
    );
    assert!(
        ModelForm {
            temperature: "  ".to_string(),
            ..ModelForm::default()
        }
        .temperature_valid()
    );
    assert!(
        !ModelForm {
            temperature: "hot".to_string(),
            ..ModelForm::default()
        }
        .temperature_valid()
    );

    let form = SettingsForm::of(&sample());
    assert!(form.max_steps_valid());
    assert!(
        !SettingsForm {
            max_steps: "201".to_string(),
            ..form.clone()
        }
        .max_steps_valid()
    );
    assert!(
        !SettingsForm {
            max_steps: "abc".to_string(),
            ..form.clone()
        }
        .max_steps_valid()
    );
    assert!(
        !SettingsForm {
            max_steps: "99999999999".to_string(),
            ..form
        }
        .max_steps_valid()
    );
}

#[test]
fn temperature_written_by_the_app_as_text_still_loads() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(PLAIN_FILE),
        r#"{"chat.baseUrl":"https://x.test/v1","chat.model":"m","chat.temperature":"0.7"}"#,
    )
    .unwrap();
    let settings = file_store(&dir).current().clone();
    assert_eq!(settings.chat.temperature, Some(0.7));
    assert_eq!(settings.chat.base_url, "https://x.test/v1");
}

#[test]
fn unknown_keys_survive_a_save() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(PLAIN_FILE), r#"{"app.keep":"yes"}"#).unwrap();
    file_store(&dir).save(sample()).unwrap();
    let plain = std::fs::read_to_string(dir.path().join(PLAIN_FILE)).unwrap();
    assert!(plain.contains("app.keep"), "{plain}");
    assert!(plain.contains("chat.model"), "{plain}");
}

#[test]
fn unreadable_secret_file_is_reset_and_reported() {
    let dir = TempDir::new().unwrap();
    file_store(&dir).save(sample()).unwrap();
    std::fs::write(dir.path().join(SECRET_FILE), "not json").unwrap();

    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&warnings);
    let store = SettingsStore::open_with(dir.path(), move |line| {
        sink.lock().unwrap().push(line.to_string())
    })
    .expect("secret file is reset");

    assert_eq!(store.current().chat.api_key, "");
    assert_eq!(store.current().operator.api_key, "");
    assert_eq!(store.current().chat.model, "doubao-chat");
    assert_eq!(store.current().max_steps, 12);
    let warnings = warnings.lock().unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains(SECRET_FILE), "{}", warnings[0]);

    // The broken file is gone: the next start is an ordinary empty secret store.
    assert_eq!(file_store(&dir).current().chat.api_key, "");
}

#[test]
fn a_plain_file_that_is_not_an_object_fails_with_its_path() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(PLAIN_FILE), "[]").unwrap();
    let error = SettingsStore::open(dir.path()).expect_err("an array is not settings");
    let message = error.to_string();
    assert!(message.contains(PLAIN_FILE), "{message}");
    assert!(message.contains("JSON object"), "{message}");
}

#[test]
fn save_keeps_the_in_memory_value_and_removes_a_cleared_temperature() {
    let mut store = in_memory_store();
    store.save(sample()).unwrap();
    assert_eq!(store.current().chat.temperature, Some(0.3));

    let without_temperature = AppSettings {
        chat: ModelSettings {
            temperature: None,
            ..sample().chat
        },
        ..sample()
    };
    store.save(without_temperature.clone()).unwrap();
    assert_eq!(store.current(), &without_temperature);
}

#[test]
fn secrets_lists_only_the_configured_keys() {
    let mut settings = sample();
    assert_eq!(settings.secrets(), vec![KEY.to_string(), KEY.to_string()]);
    settings.operator.api_key = "   ".to_string();
    assert_eq!(settings.secrets(), vec![KEY.to_string()]);
}

#[test]
fn agent_config_takes_the_two_knobs_from_the_screen() {
    let config = sample().agent_config();
    assert_eq!(config.max_steps, 12);
    assert!(!config.learn_from_success);
    let defaults = xiaoman_api::AgentConfig::default();
    assert_eq!(config.stuck_repeats, defaults.stuck_repeats);
    assert_eq!(config.max_nodes, defaults.max_nodes);
}

#[test]
fn integral_temperatures_round_trip_with_kotlins_decimal_point() {
    let settings = AppSettings {
        chat: ModelSettings {
            temperature: Some(1.0),
            ..sample().chat
        },
        ..sample()
    };
    let form = SettingsForm::of(&settings);
    assert_eq!(form.chat.temperature, "1.0");
    assert_eq!(form.to_settings(), Some(settings));
}
