//! Only launcher-selected connection/auth settings cross the isolated-home boundary.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use toml::{Table, Value};

// PIRA: 1 MiB config ceiling; raise only with matching private-handoff bounds.
const MAX_BYTES: u64 = 1024 * 1024;
const CATALOG_DECISION: &str = "selected Codex model catalog requires an authorized origin/content adapter; no catalog was loaded or requested (values withheld)";
const INVALID: &str =
    "invalid selected Codex provider/auth configuration; inspect config.toml (values withheld)";

// No Debug/Serialize: URLs, headers and query parameters may contain credentials.
#[derive(Default)]
pub struct Config {
    text: String,
    environment: Vec<String>,
    requires_native_auth: bool,
    credential_store: Option<String>,
    auth_home: Option<PathBuf>,
}

impl Config {
    pub(crate) fn load(home: Option<&Path>, has_api_key: bool) -> Result<Self, String> {
        let Some(home) = home else {
            return Self::parse("");
        };
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        // An explicit worker config is a complete selected route, not a merge of
        // the parent's transient profile/CLI overrides or other config layers.
        let worker = home.join("pira-team-worker.toml");
        let source = match fs::symlink_metadata(&worker) {
            Ok(_) => worker,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => home.join("config.toml"),
            Err(_) => return Err(INVALID.into()),
        };
        let selected_exists = fs::symlink_metadata(&source).is_ok();
        let file = match options.open(source) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !selected_exists => {
                return Self::selected(home, "", has_api_key);
            }
            Err(_) => return Err("cannot open selected Codex config.toml (values withheld)".into()),
        };
        if !file.metadata().map_err(|_| INVALID)?.is_file() {
            return Err("selected Codex config.toml must be a regular file".into());
        }
        let mut text = String::new();
        file.take(MAX_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|_| INVALID)?;
        Self::selected(home, &text, has_api_key)
    }

    fn selected(home: &Path, text: &str, has_api_key: bool) -> Result<Self, String> {
        let mut config = Self::parse(text)?;
        if config.requires_file_auth(has_api_key)
            && let Some(auth_home) = crate::authentication::selection::load(home)?
        {
            let mut output: Table = config.text.parse().map_err(|_| INVALID)?;
            output.insert(
                "cli_auth_credentials_store".into(),
                Value::String("file".into()),
            );
            config.text = toml::to_string(&output).map_err(|_| INVALID)?;
            config.credential_store = Some("file".into());
            config.auth_home = Some(auth_home);
        }
        Ok(config)
    }

    pub(crate) fn auth_home(&self) -> Option<&Path> {
        self.auth_home.as_deref()
    }
    pub(crate) fn requires_file_auth(&self, has_api_key: bool) -> bool {
        self.requires_native_auth && !has_api_key
    }

    pub(crate) fn uses_file_store(&self) -> bool {
        self.credential_store
            .as_deref()
            .is_none_or(|store| store == "file")
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        if text.len() as u64 > MAX_BYTES {
            return Err("selected Codex config.toml exceeds 1 MiB".into());
        }
        // Never include TOML parser diagnostics: they can quote credential-bearing lines.
        let source: Table = text.parse().map_err(|_| INVALID)?;
        let profile = source
            .get("profile")
            .map(|v| -> Result<&Table, String> {
                let name = v.as_str().ok_or(INVALID)?;
                source
                    .get("profiles")
                    .and_then(Value::as_table)
                    .and_then(|profiles| profiles.get(name))
                    .and_then(Value::as_table)
                    .ok_or_else(|| INVALID.into())
            })
            .transpose()?;
        let selected = |key: &str| profile.and_then(|p| p.get(key)).or_else(|| source.get(key));
        if let Some(catalog) = selected("model_catalog_json") {
            if catalog.as_str().is_none_or(str::is_empty) {
                return Err(INVALID.into());
            }
            return Err(CATALOG_DECISION.into());
        }
        let mut output = Table::new();
        for key in ["model_provider", "chatgpt_base_url"] {
            if let Some(value) = selected(key) {
                if value.as_str().is_none_or(str::is_empty) {
                    return Err(INVALID.into());
                }
                output.insert(key.into(), value.clone());
            }
        }
        for key in [
            "openai_base_url",
            "forced_login_method",
            "forced_chatgpt_workspace_id",
            "cli_auth_credentials_store",
        ] {
            if let Some(value) = source.get(key) {
                let valid = match key {
                    "forced_login_method" => matches!(value.as_str(), Some("api" | "chatgpt")),
                    "forced_chatgpt_workspace_id" => {
                        value.as_str().is_some()
                            || value
                                .as_array()
                                .is_some_and(|a| a.iter().all(|v| v.as_str().is_some()))
                    }
                    "cli_auth_credentials_store" => matches!(
                        value.as_str(),
                        Some("file" | "keyring" | "auto" | "ephemeral")
                    ),
                    _ => value.as_str().is_some(),
                };
                if !valid {
                    return Err(INVALID.into());
                }
                output.insert(key.into(), value.clone());
            }
        }
        let id = output
            .get("model_provider")
            .and_then(Value::as_str)
            .unwrap_or("openai");
        let mut requires_native_auth = id == "openai";
        let mut environment = Vec::new();
        let definitions = source
            .get("model_providers")
            .map(|value| value.as_table().ok_or(INVALID))
            .transpose()?;
        if let Some(value) = definitions.and_then(|p| p.get(id)) {
            let definition = value.as_table().ok_or(INVALID)?;
            requires_native_auth |= definition
                .get("requires_openai_auth")
                .and_then(Value::as_bool)
                == Some(true);
            let mut provider = Table::new();
            for (key, value) in definition {
                let valid = match key.as_str() {
                    "name" | "base_url" | "env_key" | "experimental_bearer_token" | "wire_api" => value.as_str().is_some(),
                    "requires_openai_auth" | "supports_websockets" | "supports_standalone_web_search" => value.as_bool().is_some(),
                    "capabilities" => provider_capabilities(value),
                    "model_catalog_url" => {
                        if value.as_str().is_none_or(str::is_empty) { return Err(INVALID.into()); }
                        return Err(CATALOG_DECISION.into());
                    },
                    "request_max_retries" | "stream_max_retries" | "stream_idle_timeout_ms" | "websocket_connect_timeout_ms" => value.as_integer().is_some_and(|n| n >= 0),
                    "http_headers" | "env_http_headers" | "query_params" => value.as_table().is_some_and(|t| t.values().all(|v| v.as_str().is_some())),
                    "env_key_instructions" => continue, // Not connection data; never inherit instructions.
                    // PIRA: auth/AWS/gateway helpers may execute commands or load additional
                    // credentials. Extend launcher auth adapters before enabling those fields.
                    // Fail closed; do not silently choose another auth route.
                    _ => return Err("unsupported selected Codex provider field; Team inherits only connection and file/environment auth settings, not executable helpers or unrecognized settings".into()),
                };
                if !valid {
                    return Err(INVALID.into());
                }
                if key == "env_key" {
                    environment.push(value.as_str().unwrap().to_owned());
                }
                if key == "env_http_headers" {
                    environment.extend(
                        value
                            .as_table()
                            .unwrap()
                            .values()
                            .map(|v| v.as_str().unwrap().to_owned()),
                    );
                }
                provider.insert(key.clone(), value.clone());
            }
            output.insert(
                "model_providers".into(),
                Value::Table(Table::from_iter([(id.to_owned(), Value::Table(provider))])),
            );
        }
        let credential_store = output
            .get("cli_auth_credentials_store")
            .and_then(Value::as_str)
            .map(str::to_owned);
        environment.sort();
        environment.dedup();
        if environment.iter().any(|name| !valid_environment_name(name)) {
            return Err("selected provider auth environment names must be identifiers and cannot replace launcher/home/temp/PATH settings".into());
        }
        Ok(Self {
            text: toml::to_string(&output).map_err(|_| INVALID)?,
            environment,
            requires_native_auth,
            credential_store,
            auth_home: None,
        })
    }

    pub(crate) fn require_supported_auth(&self, has_api_key: bool) -> Result<(), String> {
        // PIRA: isolated homes cannot share a home-keyed keyring or process login.
        // Setup explicitly selects a verified persistent file login instead.
        // Explicit API auth and unused stores need no file alias.
        if self.requires_native_auth
            && !has_api_key
            && self
                .credential_store
                .as_deref()
                .is_some_and(|store| store != "file")
        {
            return Err(format!(
                "isolated Team app-server cannot directly reuse keyring/auto/ephemeral login; {}; setup can select a separate native file login without changing main/keyring configuration",
                crate::authentication::selection::REPAIR
            ));
        }
        Ok(())
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
    pub(crate) fn environment(&self) -> &[String] {
        &self.environment
    }
    pub(crate) fn uses_environment(&self, name: &str) -> bool {
        self.environment.iter().any(|key| {
            if cfg!(windows) {
                key.eq_ignore_ascii_case(name)
            } else {
                key == name
            }
        })
    }
}

fn provider_capabilities(value: &Value) -> bool {
    value.as_table().is_some_and(|fields| {
        fields.iter().all(|(key, value)| match key.as_str() {
            "cyber_access_programs" | "external_web_access" => value.as_bool().is_some(),
            "remote_compaction" => matches!(value.as_str(), Some("unsupported" | "v2")),
            _ => false,
        })
    })
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    !upper.starts_with("PIRA_")
        && ![
            "PATH",
            "HOME",
            "USERPROFILE",
            "TMPDIR",
            "TMP",
            "TEMP",
            "TMPPREFIX",
            "CODEX_HOME",
            "CODEX_THREAD_ID",
            "CODEX_SESSION_ID",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_SHELL_PREFIX",
        ]
        .contains(&upper.as_str())
}

impl Drop for Config {
    fn drop(&mut self) {
        for byte in unsafe { self.text.as_bytes_mut() } {
            unsafe {
                std::ptr::write_volatile(byte, 0);
            }
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_only_selected_profile_route_and_auth_not_models_or_policy() {
        let config = Config::parse(
            r#"
model_provider = "unselected"
model = "must-not-change-fallback"
profile = "work"
forced_login_method = "api"
[profiles.work]
model_provider = "vendor.with.dots"
approval_policy = "on-request"
model_instructions_file = "/never-import"
[model_providers."vendor.with.dots"]
name = "Vendor"
base_url = "https://example.invalid/v1"
env_key = "SERVICE_CREDENTIAL"
http_headers = { Authorization = "synthetic-inline-header" }
env_http_headers = { "X-Custom" = "HEADER_CREDENTIAL" }
query_params = { "api-version" = "2026-01" }
wire_api = "responses"
env_key_instructions = "not forwarded"
[model_providers.unselected.auth]
command = "/never-run"
[hooks]
command = "/never-run"
[mcp_servers.bad]
command = "/never-run"
"#,
        )
        .unwrap();
        let projected: Table = config.text().parse().unwrap();
        assert_eq!(projected.len(), 3);
        assert_eq!(
            projected["model_provider"].as_str(),
            Some("vendor.with.dots")
        );
        let providers = projected["model_providers"].as_table().unwrap();
        assert_eq!(providers.len(), 1);
        assert!(!config.text().contains("never"));
        assert!(!config.text().contains("env_key_instructions"));
        assert_eq!(
            config.environment(),
            ["HEADER_CREDENTIAL", "SERVICE_CREDENTIAL"]
        );
        assert!(config.uses_environment("SERVICE_CREDENTIAL"));
        assert_eq!(config.uses_environment("service_credential"), cfg!(windows));
        assert_eq!(Config::parse(config.text()).unwrap().text(), config.text());
    }

    #[test]
    fn malformed_ambiguous_types_helpers_and_reserved_env_fail_without_values() {
        for text in [
            "model_provider = 'synthetic-secret'\nmodel_provider = 'duplicate'",
            "profile = 'synthetic-secret'",
            "model_provider = 1",
            "model_provider = 'synthetic-secret'\n[model_providers.synthetic-secret]\nauth = {command = 'never'}",
            "[model_providers.openai]\nenv_key = 'PATH'",
            "[model_providers.openai]\nmodel_catalog_url = 'https://example.invalid'",
            "model_providers = 5",
            "[model_providers.openai]\nenv_http_headers = {Authorization = 'PIRA_TEAM_CONTEXT'}",
            "[model_providers.openai]\nrequest_max_retries = -1",
            "cli_auth_credentials_store = 'keyring'",
            "model_provider = 'vendor'\ncli_auth_credentials_store = 'auto'\n[model_providers.vendor]\nrequires_openai_auth = true",
        ] {
            let error =
                match Config::parse(text).and_then(|config| config.require_supported_auth(false)) {
                    Ok(_) => panic!("unexpected success"),
                    Err(error) => error,
                };
            assert!(!error.contains("synthetic-secret"));
        }
        assert!(Config::parse("[model_providers.openai]\nbase_url = \"\"\"https://example.invalid/v1\ncontinued\"\"\"").is_ok());
        assert!(Config::parse("").unwrap().text().is_empty());
        assert!(Config::parse(&" ".repeat(MAX_BYTES as usize + 1)).is_err());
    }
}
