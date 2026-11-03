//! Native CLI adaptation boundary. Shared ownership, supervision and artifacts stay
//! outside this module; protocol loops remain native rather than emulating Codex.
//! Add a harness here, its native launch/caller adapter, and its protocol loop.
use crate::{Options, app_server, authentication};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PolicyDelivery {
    RetainedPrefix,
    ReplaceEachTurn,
}

pub(crate) struct Launch {
    pub command: Command,
    pub request_schema: Option<jsonschema::Validator>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Harness {
    Codex,
    Claude,
}

impl Harness {
    pub(crate) fn claude_stream_json(arguments: &[String]) -> bool {
        arguments
            .windows(2)
            .any(|args| args == ["--input-format", "stream-json"])
    }

    pub(crate) fn team_marker(arguments: &[String]) -> Result<Option<PathBuf>, String> {
        if Self::claude_stream_json(arguments)
            && let Some(pair) = arguments.windows(2).find(|pair| pair[0] == "--settings")
            && pair[1].starts_with('{')
        {
            let settings: Value = serde_json::from_str(&pair[1])
                .map_err(|_| "invalid Claude Team settings marker")?;
            if let Some(value) = settings["env"].get("PIRA_TEAM_CONTEXT") {
                return value
                    .as_str()
                    .map(PathBuf::from)
                    .map(Some)
                    .ok_or("invalid Claude Team context path".into());
            }
        }
        if !arguments
            .windows(3)
            .any(|a| a == ["app-server", "--strict-config", "--stdio"])
        {
            return Ok(None);
        }
        arguments
            .iter()
            .find_map(|arg| arg.strip_prefix("shell_environment_policy.set.PIRA_TEAM_CONTEXT="))
            .map(|value| {
                serde_json::from_str::<PathBuf>(value)
                    .map_err(|_| "invalid Team context marker".into())
            })
            .transpose()
    }

    pub(crate) const ALL: [Self; 2] = [Self::Codex, Self::Claude];

    pub(crate) fn inherits_native_profile(self) -> bool {
        match self {
            Self::Codex => false,
            Self::Claude => true,
        }
    }

    pub(crate) fn from_transport(transport: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.transport() == transport)
    }

    pub(crate) fn caller_identity(self, session: &str) -> String {
        match self {
            Self::Codex => session.to_owned(), // Retain published Codex owner IDs.
            Self::Claude => format!("claude-{session}"),
        }
    }

    pub(crate) fn caller_context() -> Result<Value, String> {
        // Keep native identity precedence; environment hints alone grant no authority.
        if let Ok(session) = std::env::var("CLAUDE_CODE_SESSION_ID") {
            return crate::claude::caller_context(&session);
        }
        crate::profile::codex_context()
    }

    pub(crate) fn policy_delivery(self) -> PolicyDelivery {
        match self {
            Self::Codex => PolicyDelivery::RetainedPrefix,
            Self::Claude => PolicyDelivery::ReplaceEachTurn,
        }
    }

    pub(crate) fn isolated_home(
        self,
        run: &Path,
        authentication: &authentication::Authentication,
    ) -> Result<Option<crate::IsolatedHome>, String> {
        match self {
            Self::Codex => crate::IsolatedHome::new(run, authentication).map(Some),
            Self::Claude => Ok(None), // Native CLI owns its authentication store.
        }
    }

    pub(crate) fn prepare_launch(
        self,
        options: &Options,
        run: &Path,
        dir: &Path,
        handoff: &Path,
    ) -> Result<Launch, String> {
        let request_schema = match self {
            Self::Codex => Some(crate::backend::preflight(options, run, dir)?),
            Self::Claude => None,
        };
        if crate::CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("interrupted before worker launch".into());
        }
        let command = match self {
            Self::Codex => {
                let mut command = crate::command(options, run, dir, handoff);
                crate::set_worker_launcher(&mut command)?;
                command
            }
            Self::Claude => crate::claude::command(options, run, dir, handoff)?,
        };
        Ok(Launch {
            command,
            request_schema,
        })
    }

    pub(crate) fn relevant_event(self, value: &Value) -> bool {
        match self {
            Self::Codex => app_server::codex_relevant_event(value),
            Self::Claude => crate::claude::relevant_event(value),
        }
    }

    pub(crate) fn turn(
        self,
        options: &Options,
        run: &Path,
        dir: &Path,
        task: &str,
        manifest: &mut Value,
        handoff: &Path,
        guidance: (bool, bool, bool, bool),
    ) -> Result<app_server::Turn, String> {
        match self {
            Self::Codex => {
                app_server::codex_turn::turn(options, run, dir, task, manifest, handoff, guidance)
            }
            Self::Claude => {
                app_server::claude_turn::turn(options, run, dir, task, manifest, handoff, guidance)
            }
        }
    }

    pub(crate) fn for_model(model: &str) -> Self {
        if model.starts_with("claude-") {
            Self::Claude
        } else {
            // Preserve custom Codex models and configured providers. Claude aliases
            // are deliberately not guessed: use their canonical claude-* identifier.
            Self::Codex
        }
    }

    pub(crate) fn executable(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub(crate) fn transport(self) -> &'static str {
        match self {
            Self::Codex => "app-server",
            Self::Claude => "claude-stream-json",
        }
    }

    pub(crate) fn validate_effort(self, effort: &str) -> Result<(), String> {
        if self == Self::Claude && !["low", "medium", "high", "xhigh", "max"].contains(&effort) {
            return Err(format!(
                "Claude Code does not support reasoning effort {effort}; use low, medium, high, xhigh, or max"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Harness;

    #[test]
    fn adapters_keep_native_protocols_and_owner_namespaces_separate() {
        use serde_json::json;
        for harness in Harness::ALL {
            assert_eq!(Harness::from_transport(harness.transport()), Some(harness));
        }
        assert_eq!(Harness::from_transport("unknown-transport"), None);
        assert_eq!(Harness::Codex.caller_identity("id"), "id");
        assert_eq!(Harness::Claude.caller_identity("id"), "claude-id");
        let claude = json!({"type":"result"});
        let codex = json!({"method":"turn/completed"});
        assert!(Harness::Claude.relevant_event(&claude));
        assert!(!Harness::Codex.relevant_event(&claude));
        assert!(Harness::Codex.relevant_event(&codex));
        assert!(!Harness::Claude.relevant_event(&codex));
        assert_eq!(
            crate::profile::native_caller_identity(&json!({"thread_id":"id"})),
            Some("id".into())
        );
        assert_eq!(
            crate::profile::native_caller_identity(
                &json!({"thread_id":"id","transport":"unknown"})
            ),
            None
        );
    }

    #[test]
    fn routes_families_without_version_inventory() {
        for model in ["claude-haiku-5-5", "claude-future-family-100"] {
            assert_eq!(Harness::for_model(model), Harness::Claude);
        }
        for model in ["gpt-6.1-sol", "gpt-future", "custom-provider-model"] {
            assert_eq!(Harness::for_model(model), Harness::Codex);
        }
        assert_ne!(Harness::Claude.transport(), Harness::Codex.transport());
        assert_eq!(Harness::Claude.executable(), "claude");
    }

    #[test]
    fn rejects_unrepresentable_effort_without_silent_downgrade() {
        assert!(Harness::Claude.validate_effort("ultra").is_err());
        assert!(Harness::Claude.validate_effort("high").is_ok());
        assert!(Harness::Codex.validate_effort("ultra").is_ok());
    }
}
