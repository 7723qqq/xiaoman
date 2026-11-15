//! The settings screen's rows: `SettingsScreen.kt` over `SettingsForm` from
//! `xiaoman_app::settings`.
//!
//! Parsing stays where the Kotlin app puts it (`SettingsForm.toSettings`), so this module only
//! maps the form to labelled rows and reports whether Save may run.

use xiaoman_app::model_probe::ProbeOutcome;
use xiaoman_app::settings::{AppSettings, ModelSettings, SettingsForm};

use super::copy;

/// Which endpoint a field belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Chat,
    Operator,
}

/// The text fields of one endpoint section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    BaseUrl,
    Model,
    ApiKey,
    Temperature,
}

/// One labelled text field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRow {
    pub label: String,
    /// The raw text, so editing keeps half-typed numbers and pasted keys intact.
    pub value: String,
    /// The field is red and shows [`FieldRow::help`].
    pub invalid: bool,
    pub help: Option<String>,
    /// The value is a secret: the view paints dots unless [`FieldRow::revealed`].
    pub secret: bool,
    pub revealed: bool,
}

impl FieldRow {
    /// What the field paints. A masked secret is a run of dots, never a slice of the key.
    pub fn display(&self) -> String {
        if self.secret && !self.revealed {
            "•".repeat(self.value.chars().count().min(24))
        } else {
            self.value.clone()
        }
    }
}

/// One model section: title plus its four fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRows {
    pub title: String,
    pub base_url: FieldRow,
    pub model: FieldRow,
    pub api_key: FieldRow,
    pub temperature: FieldRow,
    /// Model ids the last `获取模型` returned; empty when there is no picker to paint.
    pub models: Vec<String>,
    /// The dropdown beside the model field is open.
    pub picker_open: bool,
    /// A probe for this endpoint is in flight; its two buttons are disabled.
    pub probing: bool,
    /// The last probe's line, painted in the normal or the error colour.
    pub probe: Option<ProbeRow>,
}

/// The result line under an endpoint's probe buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRow {
    pub message: String,
    /// Success paints the message in the normal colour, failure in the error colour.
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleRow {
    pub label: String,
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonRow {
    pub label: String,
    pub enabled: bool,
}

/// Everything the settings screen paints, in Compose order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRows {
    pub title: String,
    pub chat: ModelRows,
    pub operator: ModelRows,
    pub copy_chat: String,
    pub agent_title: String,
    pub max_steps: FieldRow,
    pub learn: ToggleRow,
    pub save: ButtonRow,
    pub saved_label: Option<String>,
    pub folders_title: String,
    pub folders: Vec<String>,
}

/// The editable settings screen.
#[derive(Debug, Clone)]
pub struct SettingsModel {
    form: SettingsForm,
    saved: bool,
    reveal_chat: bool,
    reveal_operator: bool,
    models_chat: Vec<String>,
    models_operator: Vec<String>,
    picker_chat: bool,
    picker_operator: bool,
    busy_chat: bool,
    busy_operator: bool,
    probe_chat: Option<ProbeRow>,
    probe_operator: Option<ProbeRow>,
    skills_dir: String,
    memory_dir: String,
    traces_dir: String,
}

impl SettingsModel {
    pub fn new(settings: &AppSettings) -> Self {
        Self {
            form: SettingsForm::of(settings),
            saved: false,
            reveal_chat: false,
            reveal_operator: false,
            models_chat: Vec::new(),
            models_operator: Vec::new(),
            picker_chat: false,
            picker_operator: false,
            busy_chat: false,
            busy_operator: false,
            probe_chat: None,
            probe_operator: None,
            skills_dir: String::new(),
            memory_dir: String::new(),
            traces_dir: String::new(),
        }
    }

    /// The folder paths the screen lists at the bottom.
    pub fn with_folders(
        mut self,
        skills: impl Into<String>,
        memory: impl Into<String>,
        traces: impl Into<String>,
    ) -> Self {
        self.skills_dir = skills.into();
        self.memory_dir = memory.into();
        self.traces_dir = traces.into();
        self
    }

    pub fn form(&self) -> &SettingsForm {
        &self.form
    }

    pub fn saved(&self) -> bool {
        self.saved
    }

    pub fn revealed(&self, endpoint: Endpoint) -> bool {
        match endpoint {
            Endpoint::Chat => self.reveal_chat,
            Endpoint::Operator => self.reveal_operator,
        }
    }

    pub fn toggle_reveal(&mut self, endpoint: Endpoint) {
        match endpoint {
            Endpoint::Chat => self.reveal_chat = !self.reveal_chat,
            Endpoint::Operator => self.reveal_operator = !self.reveal_operator,
        }
    }

    /// The endpoint as typed, for a probe: `None` while the temperature is not a valid number.
    pub fn draft(&self, endpoint: Endpoint) -> Option<ModelSettings> {
        self.model(endpoint).to_settings()
    }

    /// The model ids the last `获取模型` returned.
    pub fn models(&self, endpoint: Endpoint) -> &[String] {
        match endpoint {
            Endpoint::Chat => &self.models_chat,
            Endpoint::Operator => &self.models_operator,
        }
    }

    /// The dropdown beside the model field is open.
    pub fn picker_open(&self, endpoint: Endpoint) -> bool {
        match endpoint {
            Endpoint::Chat => self.picker_chat,
            Endpoint::Operator => self.picker_operator,
        }
    }

    /// A probe for this endpoint is in flight.
    pub fn probing(&self, endpoint: Endpoint) -> bool {
        match endpoint {
            Endpoint::Chat => self.busy_chat,
            Endpoint::Operator => self.busy_operator,
        }
    }

    /// Marks a probe as started or finished, so its buttons disable while it runs.
    pub fn set_probing(&mut self, endpoint: Endpoint, busy: bool) {
        match endpoint {
            Endpoint::Chat => self.busy_chat = busy,
            Endpoint::Operator => self.busy_operator = busy,
        }
    }

    /// Opening or closing the model dropdown.
    pub fn toggle_picker(&mut self, endpoint: Endpoint) {
        let open = !self.picker_open(endpoint);
        self.set_picker(endpoint, open);
    }

    fn set_picker(&mut self, endpoint: Endpoint, open: bool) {
        match endpoint {
            Endpoint::Chat => self.picker_chat = open,
            Endpoint::Operator => self.picker_operator = open,
        }
    }

    /// The last probe's line for this endpoint.
    pub fn probe(&self, endpoint: Endpoint) -> Option<&ProbeRow> {
        match endpoint {
            Endpoint::Chat => self.probe_chat.as_ref(),
            Endpoint::Operator => self.probe_operator.as_ref(),
        }
    }

    /// Marks the endpoint busy and shows `message` until the probe returns.
    pub fn begin_probe(&mut self, endpoint: Endpoint, message: impl Into<String>) {
        self.set_probing(endpoint, true);
        let row = Some(ProbeRow {
            message: message.into(),
            ok: true,
        });
        match endpoint {
            Endpoint::Chat => self.probe_chat = row,
            Endpoint::Operator => self.probe_operator = row,
        }
    }

    /// One probe's result: the message always, the model list only when the outcome carries one.
    ///
    /// A successful fetch also opens the dropdown beside the model field, so the next thing the
    /// user sees is the list they asked for.
    pub fn apply_probe(&mut self, endpoint: Endpoint, outcome: ProbeOutcome) {
        self.set_probing(endpoint, false);
        if let Some(models) = outcome.models {
            let open = !models.is_empty();
            match endpoint {
                Endpoint::Chat => self.models_chat = models,
                Endpoint::Operator => self.models_operator = models,
            }
            self.set_picker(endpoint, open);
        }
        let row = Some(ProbeRow {
            message: outcome.message,
            ok: outcome.ok,
        });
        match endpoint {
            Endpoint::Chat => self.probe_chat = row,
            Endpoint::Operator => self.probe_operator = row,
        }
    }

    /// Picking a model fills the field and closes the dropdown; the list stays for another pick.
    pub fn pick_model(&mut self, endpoint: Endpoint, model: impl Into<String>) {
        self.set_field(endpoint, Field::Model, model);
        self.set_picker(endpoint, false);
    }

    /// Closes the model dropdown and forgets the fetched list.
    pub fn clear_models(&mut self, endpoint: Endpoint) {
        match endpoint {
            Endpoint::Chat => self.models_chat.clear(),
            Endpoint::Operator => self.models_operator.clear(),
        }
        self.set_picker(endpoint, false);
    }

    fn model(&self, endpoint: Endpoint) -> &xiaoman_app::settings::ModelForm {
        match endpoint {
            Endpoint::Chat => &self.form.chat,
            Endpoint::Operator => &self.form.operator,
        }
    }

    fn model_mut(&mut self, endpoint: Endpoint) -> &mut xiaoman_app::settings::ModelForm {
        match endpoint {
            Endpoint::Chat => &mut self.form.chat,
            Endpoint::Operator => &mut self.form.operator,
        }
    }

    pub fn field(&self, endpoint: Endpoint, field: Field) -> &str {
        let model = self.model(endpoint);
        match field {
            Field::BaseUrl => &model.base_url,
            Field::Model => &model.model,
            Field::ApiKey => &model.api_key,
            Field::Temperature => &model.temperature,
        }
    }

    /// Typing into a field. Every edit hides the "已保存" note, as Compose does.
    pub fn set_field(&mut self, endpoint: Endpoint, field: Field, value: impl Into<String>) {
        let value = value.into();
        let model = self.model_mut(endpoint);
        match field {
            Field::BaseUrl => model.base_url = value,
            Field::Model => model.model = value,
            Field::ApiKey => model.api_key = value,
            Field::Temperature => model.temperature = value,
        }
        self.saved = false;
    }

    pub fn temperature_valid(&self, endpoint: Endpoint) -> bool {
        self.model(endpoint).temperature_valid()
    }

    pub fn max_steps(&self) -> &str {
        &self.form.max_steps
    }

    pub fn set_max_steps(&mut self, value: impl Into<String>) {
        self.form.max_steps = value.into();
        self.saved = false;
    }

    pub fn max_steps_valid(&self) -> bool {
        self.form.max_steps_valid()
    }

    pub fn learn(&self) -> bool {
        self.form.learn_from_success
    }

    pub fn set_learn(&mut self, on: bool) {
        self.form.learn_from_success = on;
        self.saved = false;
    }

    pub fn toggle_learn(&mut self) {
        self.set_learn(!self.learn());
    }

    /// `与聊天模型相同`: copies the chat base URL and API key onto the operator endpoint.
    pub fn copy_chat_to_operator(&mut self) {
        self.form.operator.base_url = self.form.chat.base_url.clone();
        self.form.operator.api_key = self.form.chat.api_key.clone();
        self.saved = false;
    }

    /// `None` while any field is invalid, exactly like `SettingsForm.toSettings`.
    pub fn to_settings(&self) -> Option<AppSettings> {
        self.form.to_settings()
    }

    /// The Save button: enabled only when the form parses.
    pub fn can_save(&self) -> bool {
        self.to_settings().is_some()
    }

    /// What Save does: the parsed settings, with the "已保存" note turned on.
    pub fn save(&mut self) -> Option<AppSettings> {
        let settings = self.to_settings()?;
        self.saved = true;
        Some(settings)
    }

    pub fn rows(&self) -> SettingsRows {
        let chat = self.model_rows(Endpoint::Chat, copy::SETTINGS_CHAT_MODEL);
        let operator = self.model_rows(Endpoint::Operator, copy::SETTINGS_OPERATOR_MODEL);
        let mut folders = Vec::new();
        if !self.skills_dir.is_empty() {
            folders.push(copy::settings_folder_skills(&self.skills_dir));
        }
        if !self.memory_dir.is_empty() {
            folders.push(copy::settings_folder_memory(&self.memory_dir));
        }
        if !self.traces_dir.is_empty() {
            folders.push(copy::settings_folder_traces(&self.traces_dir));
        }
        SettingsRows {
            title: copy::SETTINGS_TITLE.to_string(),
            chat,
            operator,
            copy_chat: copy::SETTINGS_COPY_CHAT.to_string(),
            agent_title: copy::SETTINGS_AGENT.to_string(),
            max_steps: FieldRow {
                label: copy::SETTINGS_MAX_STEPS.to_string(),
                value: self.form.max_steps.clone(),
                invalid: !self.form.max_steps_valid(),
                help: (!self.form.max_steps_valid())
                    .then(|| copy::SETTINGS_INVALID_NUMBER.to_string()),
                secret: false,
                revealed: true,
            },
            learn: ToggleRow {
                label: copy::SETTINGS_LEARN.to_string(),
                on: self.form.learn_from_success,
            },
            save: ButtonRow {
                label: copy::SETTINGS_SAVE.to_string(),
                enabled: self.can_save(),
            },
            saved_label: self.saved.then(|| copy::SETTINGS_SAVED.to_string()),
            folders_title: copy::SETTINGS_FOLDERS.to_string(),
            folders,
        }
    }

    fn model_rows(&self, endpoint: Endpoint, title: &str) -> ModelRows {
        let revealed = self.revealed(endpoint);
        let temperature_valid = self.temperature_valid(endpoint);
        ModelRows {
            title: title.to_string(),
            base_url: FieldRow {
                label: copy::SETTINGS_BASE_URL.to_string(),
                value: self.field(endpoint, Field::BaseUrl).to_string(),
                invalid: false,
                help: None,
                secret: false,
                revealed: true,
            },
            model: FieldRow {
                label: copy::SETTINGS_MODEL.to_string(),
                value: self.field(endpoint, Field::Model).to_string(),
                invalid: false,
                help: None,
                secret: false,
                revealed: true,
            },
            api_key: FieldRow {
                label: copy::SETTINGS_API_KEY.to_string(),
                value: self.field(endpoint, Field::ApiKey).to_string(),
                invalid: false,
                help: None,
                secret: true,
                revealed,
            },
            temperature: FieldRow {
                label: copy::SETTINGS_TEMPERATURE.to_string(),
                value: self.field(endpoint, Field::Temperature).to_string(),
                invalid: !temperature_valid,
                help: (!temperature_valid).then(|| copy::SETTINGS_INVALID_NUMBER.to_string()),
                secret: false,
                revealed: true,
            },
            models: self.models(endpoint).to_vec(),
            picker_open: self.picker_open(endpoint),
            probing: self.probing(endpoint),
            probe: self.probe(endpoint).cloned(),
        }
    }
}
