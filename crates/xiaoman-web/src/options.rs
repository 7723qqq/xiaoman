//! Server settings, mirroring `WebServer.Options` of the Kotlin version.

use std::path::PathBuf;

use crate::claims::ClaimConfig;
use crate::class_gateway::ClassOptions;
use crate::class_tokens::zone_offset;
use crate::ics::DEFAULT_OFFSET_MINUTES;

#[derive(Debug, Clone)]
pub struct WebOptions {
    pub host: String,
    pub port: u16,
    /// Directory served for everything outside `/api/`; `None` uses the bundled page.
    pub static_root: Option<PathBuf>,
    /// Shared secret every `/api/` route except `/api/info` and `/api/claim/*` requires.
    pub access_code: Option<String>,
    /// Root with skill folders; only chat workflows reach web sessions.
    pub skills_root: Option<PathBuf>,
    /// Allows private addresses for both model and MCP endpoints (local tests only).
    pub allow_private: bool,
    /// Offset of the display zone from UTC, in minutes (`Asia/Shanghai` is 480).
    pub timezone_offset_minutes: i32,
    pub session_millis: i64,
    pub max_sessions: usize,
    pub chat_timeout_millis: u64,
    pub heartbeat_millis: u64,
    /// Lets catalog stdio servers run as child processes; off means the catalog hides them.
    pub stdio_mcp: bool,
    /// Global cap on concurrently running stdio MCP child processes (`--max-stdio`, default 20).
    pub max_stdio: usize,
    /// `--local`: serves `/connect` and its `/connect/info` JSON to this computer only.
    pub local_connect: bool,
    /// Class-token mode (`XIAOMAN_CLASS_*`); `None` keeps it off.
    pub class: Option<ClassOptions>,
    /// Self-service claiming through the forum (`XIAOMAN_CLAIM_*`); needs `class`.
    pub claim: Option<ClaimConfig>,
}

impl Default for WebOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8787,
            static_root: None,
            access_code: None,
            skills_root: None,
            allow_private: false,
            timezone_offset_minutes: DEFAULT_OFFSET_MINUTES,
            session_millis: 7_200_000,
            max_sessions: 500,
            chat_timeout_millis: 300_000,
            heartbeat_millis: 10_000,
            stdio_mcp: false,
            max_stdio: 20,
            local_connect: false,
            class: None,
            claim: None,
        }
    }
}

impl WebOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.session_millis <= 0 {
            return Err("session_millis must be positive".to_string());
        }
        if self.max_sessions == 0 {
            return Err("max_sessions must be positive".to_string());
        }
        if self.chat_timeout_millis == 0 {
            return Err("chat_timeout_millis must be positive".to_string());
        }
        if self.heartbeat_millis == 0 {
            return Err("heartbeat_millis must be positive".to_string());
        }
        if self.max_stdio == 0 {
            return Err("max_stdio must be positive".to_string());
        }
        if self.timezone_offset_minutes.abs() > 24 * 60 {
            return Err("timezone_offset_minutes is out of range".to_string());
        }
        if let Some(class) = &self.class {
            class.validate()?;
        }
        if let Some(claim) = &self.claim {
            // Claiming writes tokens into the class-token file, so it needs that mode.
            if self.class.is_none() {
                return Err("自助领取需要先开启课堂令牌".to_string());
            }
            claim.validate()?;
        }
        Ok(())
    }

    pub fn access_code_required(&self) -> bool {
        self.access_code
            .as_deref()
            .map(|code| !code.is_empty())
            .unwrap_or(false)
    }

    pub fn static_root_or_bundled(&self) -> PathBuf {
        if let Some(root) = &self.static_root {
            return root.clone();
        }
        // Installed layout: `<dir>/bin/web-server` next to `<dir>/resources/web`.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for candidate in [dir.join("resources/web"), dir.join("../resources/web")] {
                    if candidate.is_dir() {
                        return candidate;
                    }
                }
            }
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/web")
    }

    /// Reads the environment the Kotlin server used: the access code and skills root, the zone, and
    /// the two class-token blocks. Host, port and the remaining switches come from the caller.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<WebOptions, String> {
        Self::from_env_with(env, None)
    }

    /// [`WebOptions::from_env`] with `--class-tokens FILE` already known.
    pub fn from_env_with(
        env: &dyn Fn(&str) -> Option<String>,
        class_tokens: Option<PathBuf>,
    ) -> Result<WebOptions, String> {
        let mut options = WebOptions::default();
        if let Some(code) = env("XIAOMAN_WEB_ACCESS_CODE").filter(|code| !code.trim().is_empty()) {
            options.access_code = Some(code.trim().to_string());
        }
        if let Some(root) = env("XIAOMAN_WEB_SKILLS")
            .map(|root| root.trim().to_string())
            .filter(|root| !root.is_empty())
        {
            options.skills_root = Some(PathBuf::from(root));
        }
        if let Some(zone) = env("XIAOMAN_TZ")
            .map(|zone| zone.trim().to_string())
            .filter(|zone| !zone.is_empty())
        {
            options.timezone_offset_minutes =
                zone_offset(&zone).ok_or_else(|| format!("不支持的时区 {zone}"))?;
        }
        let (class, _class_status) = ClassOptions::from_env(env, class_tokens)?;
        options.class = class;
        if options.class.is_some() {
            let (claim, _claim_status) = ClaimConfig::from_env(
                env,
                options.timezone_offset_minutes,
                xiaoman_api::now_millis(),
            )?;
            options.claim = claim;
        }
        options.validate()?;
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_kotlin_server() {
        let options = WebOptions::default();
        assert_eq!(options.host, "127.0.0.1");
        assert_eq!(options.port, 8787);
        assert_eq!(options.timezone_offset_minutes, DEFAULT_OFFSET_MINUTES);
        assert!(!options.access_code_required());
        assert!(!options.allow_private);
        assert!(options.class.is_none() && options.claim.is_none());
        assert!(
            options
                .static_root_or_bundled()
                .join("index.html")
                .is_file()
        );
        assert_eq!(
            WebOptions {
                max_sessions: 0,
                ..Default::default()
            }
            .validate(),
            Err("max_sessions must be positive".to_string())
        );
    }

    #[test]
    fn the_environment_turns_class_tokens_and_claiming_on() {
        let vars = |name: &str| match name {
            "XIAOMAN_CLASS_UPSTREAM_KEY" => Some("secret".to_string()),
            "XIAOMAN_CLASS_MODELS" => Some("xiaoman-class=doubao-real".to_string()),
            "XIAOMAN_CLASS_TOKENS" => Some("/tmp/class-tokens.json".to_string()),
            "XIAOMAN_CLAIM_TOPIC_ID" => Some("https://forum.example/t/claim/465300/3".to_string()),
            "XIAOMAN_CLAIM_FORUM" => Some("https://forum.example".to_string()),
            "XIAOMAN_CLAIM_EXPIRES" => Some("2026-11-12 21:00".to_string()),
            _ => None,
        };
        let options = WebOptions::from_env(&vars).expect("valid environment");
        let class = options.class.expect("class tokens are on");
        assert_eq!(class.upstream_key, "secret");
        assert_eq!(class.tokens_file, PathBuf::from("/tmp/class-tokens.json"));
        let claim = options.claim.expect("claiming is on");
        assert_eq!(claim.topic_id, 465300);
        assert_eq!(claim.topic_url(), "https://forum.example/t/465300");
        // Claiming without class tokens is a configuration error, not a silent no-op.
        let broken = WebOptions {
            class: None,
            claim: Some(claim),
            ..Default::default()
        };
        assert_eq!(
            broken.validate(),
            Err("自助领取需要先开启课堂令牌".to_string())
        );
    }

    #[test]
    fn a_broken_claim_setting_is_reported() {
        let vars = |name: &str| match name {
            "XIAOMAN_CLASS_UPSTREAM_KEY" => Some("secret".to_string()),
            "XIAOMAN_CLASS_MODELS" => Some("xiaoman-class=doubao-real".to_string()),
            "XIAOMAN_CLAIM_TOPIC_ID" => Some("465300".to_string()),
            _ => None,
        };
        assert_eq!(
            WebOptions::from_env(&vars).unwrap_err(),
            "开启自助领取需要 XIAOMAN_CLAIM_EXPIRES（如 2026-11-12 21:00）"
        );
    }
}
