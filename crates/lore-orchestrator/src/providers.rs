//! Model providers: other models a Codex run can go through, such as a local
//! Ollama or LM Studio, or any endpoint that speaks the OpenAI Responses API.
//!
//! Lore never talks to a provider itself. It hands Codex the provider's
//! settings for one run and Codex makes the calls, as it does for OpenAI.
//! Settings live in `providers.json`; API keys live in the system keychain and
//! reach the agent only through its environment, never its arguments.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use lore_ipc::{ProviderDto, ProviderKind, SaveProviderRequest};
use serde::{Deserialize, Serialize};

use crate::{agent, Error, Result};

pub(crate) const PROVIDERS_FILE: &str = "providers.json";
const MAX_NAME: usize = 60;
const MAX_URL: usize = 500;
const MAX_MODELS: usize = 50;
const MAX_KEY: usize = 4_096;

/// A provider as saved in `providers.json`. The key is not in here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProviderRecord {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    /// Whether a key was saved, kept here so listing providers never has to
    /// read the keychain (which can prompt).
    #[serde(default)]
    pub has_api_key: bool,
}

impl ProviderRecord {
    pub fn dto(&self) -> ProviderDto {
        ProviderDto {
            id: self.id.clone(),
            name: self.name.clone(),
            kind: self.kind,
            base_url: self.base_url.clone(),
            models: self.models.clone(),
            has_api_key: self.has_api_key,
        }
    }
}

/// What one Codex run needs to go through a provider.
#[derive(Clone)]
pub struct ProviderLaunch {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

impl std::fmt::Debug for ProviderLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderLaunch")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Where provider API keys are kept.
pub trait SecretStore: Send + Sync {
    fn get(&self, provider: &str) -> Result<Option<String>>;
    fn set(&self, provider: &str, key: &str) -> Result<()>;
    fn delete(&self, provider: &str) -> Result<()>;
}

/// The system keychain: one generic password per provider.
pub struct Keychain;

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "dev.lore.app.providers";
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25_300;

#[cfg(target_os = "macos")]
impl SecretStore for Keychain {
    fn get(&self, provider: &str) -> Result<Option<String>> {
        match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, provider) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| Error::Invalid("the saved API key is not valid text".into())),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(Error::Invalid(format!("the keychain refused: {e}"))),
        }
    }

    fn set(&self, provider: &str, key: &str) -> Result<()> {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            provider,
            key.as_bytes(),
        )
        .map_err(|e| Error::Invalid(format!("the keychain refused: {e}")))
    }

    fn delete(&self, provider: &str) -> Result<()> {
        match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, provider) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(Error::Invalid(format!("the keychain refused: {e}"))),
        }
    }
}

/// Lore ships for macOS; elsewhere keys cannot be saved yet.
#[cfg(not(target_os = "macos"))]
impl SecretStore for Keychain {
    fn get(&self, _provider: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn set(&self, _provider: &str, _key: &str) -> Result<()> {
        Err(Error::Invalid(
            "saving an API key needs the macOS Keychain".into(),
        ))
    }

    fn delete(&self, _provider: &str) -> Result<()> {
        Ok(())
    }
}

/// Keys held in memory, for tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, provider: &str) -> Result<Option<String>> {
        Ok(self.lock().get(provider).cloned())
    }

    fn set(&self, provider: &str, key: &str) -> Result<()> {
        self.lock().insert(provider.to_string(), key.to_string());
        Ok(())
    }

    fn delete(&self, provider: &str) -> Result<()> {
        self.lock().remove(provider);
        Ok(())
    }
}

impl MemorySecrets {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Every saved provider; an unreadable file reads as none.
pub(crate) fn load(root: &Path) -> Vec<ProviderRecord> {
    fs::read(root.join(PROVIDERS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub(crate) fn store(root: &Path, all: &[ProviderRecord]) -> Result<()> {
    let tmp = root.join(format!("{PROVIDERS_FILE}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(all)?)?;
    fs::rename(tmp, root.join(PROVIDERS_FILE))?;
    Ok(())
}

/// The provider record `req` describes, checked: a name, a base URL for a
/// custom provider, and at least one model id Lore can pass as `--model`.
pub(crate) fn validate(
    req: &SaveProviderRequest,
    existing: &[ProviderRecord],
) -> Result<ProviderRecord> {
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(Error::Invalid(format!(
            "provider name must be 1-{MAX_NAME} printable characters"
        )));
    }
    let base_url = match req.kind {
        ProviderKind::Custom => Some(base_url(req.base_url.as_deref().unwrap_or(""))?),
        ProviderKind::Ollama | ProviderKind::Lmstudio => None,
    };
    let mut models: Vec<String> = Vec::new();
    for raw in &req.models {
        if let Some(model) = agent::parse_model(Some(raw))? {
            if !models.contains(&model) {
                models.push(model);
            }
        }
    }
    if models.is_empty() || models.len() > MAX_MODELS {
        return Err(Error::Invalid(format!(
            "add 1-{MAX_MODELS} model ids, such as gpt-oss:20b or qwen/qwen3-coder"
        )));
    }
    let id = match req.id.as_deref() {
        Some(id) => existing
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.id.clone())
            .ok_or_else(|| Error::Invalid("that provider no longer exists".into()))?,
        None => unique_id(name, existing),
    };
    let had_key = existing.iter().any(|p| p.id == id && p.has_api_key);
    Ok(ProviderRecord {
        id,
        name: name.to_string(),
        kind: req.kind,
        base_url,
        models,
        has_api_key: had_key && req.kind == ProviderKind::Custom,
    })
}

/// A new key to save (`Some(Some(key))`), one to remove (`Some(None)`), or no
/// change (`None`). Local providers never take a key.
pub(crate) fn key_change(req: &SaveProviderRequest) -> Result<Option<Option<String>>> {
    if req.kind != ProviderKind::Custom {
        return Ok(Some(None));
    }
    match req.api_key.as_deref().map(str::trim) {
        None => Ok(None),
        Some("") => Ok(Some(None)),
        Some(key) => {
            if key.len() > MAX_KEY || key.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err(Error::Invalid(
                    "the API key must be one line of printable characters".into(),
                ));
            }
            Ok(Some(Some(key.to_string())))
        }
    }
}

/// An `http(s)://` URL with nothing that could break out of a TOML string or
/// an argument.
fn base_url(raw: &str) -> Result<String> {
    let url = raw.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    let ok = rest.is_some_and(|r| !r.is_empty())
        && url.len() <= MAX_URL
        && !url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\'' | '\\'));
    if !ok {
        return Err(Error::Invalid(
            "base URL must be an http:// or https:// address, such as https://openrouter.ai/api/v1"
                .into(),
        ));
    }
    Ok(url.to_string())
}

/// A short id from the name, unique among `existing`: lowercase letters,
/// digits, and dashes, so it is safe in a Codex config key.
fn unique_id(name: &str, existing: &[ProviderRecord]) -> String {
    let mut base = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            base.push(c.to_ascii_lowercase());
        } else if !base.ends_with('-') && !base.is_empty() {
            base.push('-');
        }
    }
    let base = base
        .trim_end_matches('-')
        .chars()
        .take(32)
        .collect::<String>();
    let base = if base.is_empty() {
        "provider".to_string()
    } else {
        base
    };
    let taken = |id: &str| existing.iter().any(|p| p.id == id);
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .unwrap_or(base)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn req(kind: ProviderKind, base_url: Option<&str>, models: &[&str]) -> SaveProviderRequest {
        SaveProviderRequest {
            id: None,
            name: "Open Router".into(),
            kind,
            base_url: base_url.map(str::to_string),
            models: models.iter().map(|m| m.to_string()).collect(),
            api_key: None,
        }
    }

    #[test]
    fn validates_names_urls_and_models() {
        let ok = validate(
            &req(
                ProviderKind::Custom,
                Some(" https://openrouter.ai/api/v1/ "),
                &["qwen/qwen3-coder", "qwen/qwen3-coder", " "],
            ),
            &[],
        )
        .unwrap();
        assert_eq!(ok.id, "open-router");
        assert_eq!(ok.base_url.as_deref(), Some("https://openrouter.ai/api/v1"));
        assert_eq!(ok.models, ["qwen/qwen3-coder"]);

        let local = validate(
            &req(
                ProviderKind::Ollama,
                Some("https://ignored"),
                &["gpt-oss:20b"],
            ),
            &[],
        )
        .unwrap();
        assert_eq!(local.base_url, None);

        for bad_url in [
            "",
            "ftp://x",
            "https://",
            "https://a b",
            "https://x\"y",
            "javascript:alert(1)",
        ] {
            assert!(
                validate(&req(ProviderKind::Custom, Some(bad_url), &["m"]), &[]).is_err(),
                "{bad_url}"
            );
        }
        assert!(validate(&req(ProviderKind::Ollama, None, &[]), &[]).is_err());
        assert!(validate(
            &req(ProviderKind::Ollama, None, &["--dangerously-bypass"]),
            &[]
        )
        .is_err());
        let mut unnamed = req(ProviderKind::Ollama, None, &["m"]);
        unnamed.name = " ".into();
        assert!(validate(&unnamed, &[]).is_err());
    }

    #[test]
    fn ids_are_unique_and_updates_keep_theirs() {
        let first = validate(&req(ProviderKind::Ollama, None, &["m"]), &[]).unwrap();
        let second = validate(
            &req(ProviderKind::Ollama, None, &["m"]),
            std::slice::from_ref(&first),
        )
        .unwrap();
        assert_eq!(
            (first.id.as_str(), second.id.as_str()),
            ("open-router", "open-router-2")
        );

        let mut update = req(ProviderKind::Ollama, None, &["n"]);
        update.id = Some("open-router".into());
        assert_eq!(
            validate(&update, std::slice::from_ref(&first)).unwrap().id,
            "open-router"
        );
        update.id = Some("gone".into());
        assert!(validate(&update, &[first]).is_err());
    }

    #[test]
    fn key_changes() {
        let mut custom = req(ProviderKind::Custom, Some("https://x.dev/v1"), &["m"]);
        assert_eq!(key_change(&custom).unwrap(), None);
        custom.api_key = Some(" sk-abc ".into());
        assert_eq!(key_change(&custom).unwrap(), Some(Some("sk-abc".into())));
        custom.api_key = Some(String::new());
        assert_eq!(key_change(&custom).unwrap(), Some(None));
        custom.api_key = Some("two words".into());
        assert!(key_change(&custom).is_err());
        let mut local = req(ProviderKind::Lmstudio, None, &["m"]);
        local.api_key = Some("sk-abc".into());
        assert_eq!(key_change(&local).unwrap(), Some(None));
    }

    #[test]
    fn keys_never_show_in_debug_output() {
        let launch = ProviderLaunch {
            id: "x".into(),
            name: "X".into(),
            kind: ProviderKind::Custom,
            base_url: None,
            api_key: Some("sk-secret".into()),
        };
        assert!(!format!("{launch:?}").contains("sk-secret"));
        let mut r = req(ProviderKind::Custom, None, &[]);
        r.api_key = Some("sk-secret".into());
        assert!(!format!("{r:?}").contains("sk-secret"));
    }
}
