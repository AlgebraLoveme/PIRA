//! Run intent and running-turn publication; never a claim about actual billing.
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tier {
    Standard,
    Fast,
}

impl Tier {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "standard" => Ok(Self::Standard),
            "fast" => Ok(Self::Fast),
            _ => Err("service tier must be standard or fast".into()),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Fast => "fast",
        }
    }

    pub(crate) fn backend(self) -> &'static str {
        match self {
            Self::Standard => "default",
            Self::Fast => "fast",
        }
    }

    pub(crate) fn retained(manifest: &Value) -> Result<Self, String> {
        match manifest.get("service_tier") {
            None => Ok(Self::Standard),
            Some(value) => Self::parse(value.as_str().ok_or("invalid persisted service_tier")?),
        }
    }

    pub(crate) fn update(self, thread: &str, turn: &str) -> Value {
        json!({"threadId":thread,"turnId":turn,"serviceTier":self.backend()})
    }
}

pub(crate) fn active_update(
    request: &Value,
    thread: &str,
    turn: &str,
    stopping: bool,
    pending: bool,
) -> Result<(Tier, Value), String> {
    if request["turn_id"] != turn || stopping {
        return Err("no matching active turn; no tier was changed".into());
    }
    if pending {
        return Err("tier publication already pending; await its outcome".into());
    }
    let tier = Tier::parse(
        request["service_tier"]
            .as_str()
            .ok_or("missing service_tier")?,
    )?;
    Ok((tier, tier.update(thread, turn)))
}

pub(crate) fn publication(
    tier: Tier,
    turn: &str,
    response: &Value,
    manifest: &mut Value,
    persist: impl FnOnce(&Value) -> Result<(), String>,
) -> Value {
    if let Some(error) = response.get("error") {
        return json!({"status":"rejected","publication":"rejected","error":error,"actual_service_tier":null});
    }
    match response["result"]["status"].as_str() {
        Some("targetUnavailable") => json!({"status":"rejected","publication":"targetUnavailable",
            "error":"target turn unavailable; desired run tier unchanged","actual_service_tier":null}),
        Some("applied") => {
            manifest["service_tier"] = json!(tier.name());
            manifest["tier_publication"] = json!({"status":"applied","tier":tier.name(),"turn_id":turn,
                "revision":manifest["revision"],"actual_service_tier":null});
            let saved = persist(manifest);
            let mut receipt = json!({"status":"accepted","publication":"applied","desired_service_tier":tier.name(),
                "published_service_tier":tier.name(),"turn_id":turn,"revision":manifest["revision"],"actual_service_tier":null});
            if let Err(error) = saved {
                receipt["status"] = json!("unknown");
                receipt["error"] = json!(format!(
                    "backend publication applied but persistence failed: {error}; do not blindly retry"
                ));
            }
            receipt
        }
        _ => json!({"status":"unknown","publication":"unknown",
            "error":"missing or unrecognized publication acknowledgement; desired run tier unchanged; do not blindly retry",
            "actual_service_tier":null}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_is_explicit_despite_parent_fast_and_resume_retains_intent() {
        let options = crate::parse_using(&["run","--task","test","--completion-gate","test"]
            .map(str::to_owned), || Ok(json!({"service_tier":"fast","approval_policy":"never",
                "sandbox_policy":{"type":"danger-full-access"},"permission_profile":{"type":"disabled"}}))).unwrap();
        assert_eq!(options.service_tier, Tier::Standard);
        assert_eq!(options.service_tier.backend(), "default");
        assert_eq!(
            Tier::retained(&json!({"service_tier":"fast"})).unwrap(),
            Tier::Fast
        );
        assert_eq!(Tier::retained(&json!({})).unwrap(), Tier::Standard);
        assert!(Tier::retained(&json!({"service_tier":"inherited"})).is_err());
        assert!(Tier::parse("default").is_err());
    }

    #[test]
    fn live_update_is_tier_only_and_rejects_busy_stale_stopping() {
        for (name, backend) in [("standard", "default"), ("fast", "fast")] {
            let request = json!({"turn_id":"turn","service_tier":name});
            let (_, params) = active_update(&request, "thread", "turn", false, false).unwrap();
            assert_eq!(
                params,
                json!({"threadId":"thread","turnId":"turn","serviceTier":backend})
            );
            assert!(active_update(&request, "thread", "other", false, false).is_err());
            assert!(active_update(&request, "thread", "turn", true, false).is_err());
            assert!(active_update(&request, "thread", "turn", false, true).is_err());
        }
    }

    #[test]
    fn applied_persists_intent_without_claiming_effective_tier() {
        let mut manifest = json!({"revision":2,"service_tier":"standard"});
        let response = publication(
            Tier::Fast,
            "turn",
            &json!({"result":{"status":"applied"}}),
            &mut manifest,
            |state| {
                assert_eq!(state["service_tier"], "fast");
                Ok(())
            },
        );
        assert_eq!(response["status"], "accepted");
        assert_eq!(response["publication"], "applied");
        assert!(response["actual_service_tier"].is_null());
        assert_eq!(manifest["tier_publication"]["turn_id"], "turn");
    }

    #[test]
    fn rejection_unavailable_and_unknown_never_commit_future_intent() {
        for (reply, status) in [
            (
                json!({"error":{"code":-1,"message":"unsupported"}}),
                "rejected",
            ),
            (json!({"result":{"status":"targetUnavailable"}}), "rejected"),
            (json!({"result":{}}), "unknown"),
        ] {
            let mut manifest = json!({"revision":1,"service_tier":"standard"});
            let before = manifest.clone();
            let result = publication(Tier::Fast, "turn", &reply, &mut manifest, |_| {
                panic!("must not persist")
            });
            assert_eq!(result["status"], status);
            assert_eq!(manifest, before);
        }
    }

    #[test]
    fn publication_persistence_error_is_control_local_and_not_a_false_rejection() {
        let mut manifest = json!({"service_tier":"standard","revision":1});
        let result = publication(
            Tier::Fast,
            "turn",
            &json!({"result":{"status":"applied"}}),
            &mut manifest,
            |_| Err("fixture disk error".into()),
        );
        assert_eq!(result["status"], "unknown");
        assert_eq!(result["publication"], "applied");
        assert_eq!(manifest["service_tier"], "fast");
    }
    #[test]
    fn explicit_tier_cli_values_are_validated_and_process_feature_is_local() {
        for (name, expected) in [("standard", Tier::Standard), ("fast", Tier::Fast)] {
            let options = crate::tests::options(&["task", "--service-tier", name]).unwrap();
            assert_eq!(options.service_tier, expected);
            let args: Vec<_> = crate::command(
                &options,
                std::path::Path::new("/fixture/run"),
                std::path::Path::new("/fixture/revision"),
                std::path::Path::new("/fixture/handoff"),
            )
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
            assert!(args.iter().any(|arg| arg == "features.fast_mode=true"));
        }
        assert!(crate::tests::options(&["task", "--service-tier", "other"]).is_err());
        assert!(
            crate::tests::options(&[
                "task",
                "--service-tier",
                "fast",
                "--service-tier",
                "standard"
            ])
            .is_err()
        );
    }
}
