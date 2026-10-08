//! Native API inventory plus validation of the exact requests Team sends.
use crate::{Options, create, isolate, terminate};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const CONTRACT: &str = include_str!("backend_contract.json");
const GUIDE: &str = "Install/update the native Codex CLI on this execution host (https://developers.openai.com/codex/app-server/); Team requires the published app-server API and strict-config support. No model call was started by this preflight.";

pub fn contract() -> Value {
    serde_json::from_str(CONTRACT).expect("embedded backend contract")
}

pub fn preflight(
    options: &Options,
    run: &Path,
    dir: &Path,
) -> Result<jsonschema::Validator, String> {
    let check = || -> Result<jsonschema::Validator, String> {
        let output = dir.join("backend-schema");
        fs::create_dir(&output).map_err(|e| format!("create backend schema directory: {e}"))?;
        let mut cmd = Command::new("codex");
        cmd.args(["app-server", "generate-json-schema", "--out"])
            .arg(&output)
            .env("CODEX_HOME", run.join("codex-home"))
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(create(&dir.join("backend-check.log"))?);
        isolate(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("generate native Codex schema: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None)
                    if Instant::now() < deadline
                        && !crate::CANCELLED.load(std::sync::atomic::Ordering::SeqCst) =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                result => {
                    terminate(child.id());
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "native schema generation interrupted/timed out or failed: {result:?}"
                    ));
                }
            }
        };
        if !status.success() {
            return Err(format!(
                "native schema generation failed ({status}); see backend-check.log"
            ));
        }
        let schemas = contract()["schemas"].as_object().unwrap().clone();
        let mut request = None;
        for (file, expected) in schemas {
            let path = output.join(&file);
            let meta = fs::symlink_metadata(&path).map_err(|e| format!("{file}: {e}"))?;
            if !meta.is_file() || meta.len() > 16 * 1024 * 1024 {
                return Err(format!("{file}: expected a regular schema file <=16 MiB"));
            }
            let mut schema: Value =
                serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("{file}: invalid schema JSON: {e}"))?;
            inventory(&schema, &expected).map_err(|e| format!("{file}: {e}"))?;
            if file == "ClientNotification.json" {
                let validator = jsonschema::validator_for(&schema)
                    .map_err(|e| format!("unsupported native notification schema: {e}"))?;
                validate_request(&validator, &json!({"method":"initialized"}))?;
            }
            if file == "ClientRequest.json" {
                published_fields_only(&mut schema);
                request = Some(
                    jsonschema::options()
                        .build(&schema)
                        .map_err(|e| format!("unsupported native request schema: {e}"))?,
                );
            }
        }
        let request = request.ok_or("missing native request schema")?;
        validate_examples(&request, options)?;
        Ok(request)
    };
    check().map_err(|e| format!("unsupported Codex backend: {e}. {GUIDE}"))
}

pub fn validate_request(validator: &jsonschema::Validator, request: &Value) -> Result<(), String> {
    validator.validate(request).map_err(|error| format!(
        "unsupported Codex {} request at {} (native schema {}); update Codex/Team to compatible builds",
        request["method"].as_str().unwrap_or("RPC"), error.instance_path, error.schema_path
    ))
}

fn validate_examples(validator: &jsonschema::Validator, options: &Options) -> Result<(), String> {
    let id = "00000000-0000-0000-0000-000000000001";
    let mut thread = json!({"model":options.model,"cwd":options.cwd,"sandbox":options.execution.mode,"config":options.execution.config(),
        "approvalPolicy":"never","baseInstructions":"probe","developerInstructions":""});
    let text = json!([{"type":"text","text":"probe"}]);
    let mut examples = vec![
        (
            "initialize",
            json!({"clientInfo":{"name":"pira_team","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":false}}),
        ),
        (
            "thread/inject_items",
            json!({"threadId":id,"items":[{"type":"message","role":"developer","content":[{"type":"input_text","text":"probe"}]}]}),
        ),
        (
            "turn/start",
            json!({"threadId":id,"input":text,"model":options.model,"effort":options.effort,
            "cwd":options.cwd,"approvalPolicy":"never","sandboxPolicy":options.execution.sandbox}),
        ),
        (
            "turn/steer",
            json!({"threadId":id,"expectedTurnId":id,"input":text}),
        ),
        ("turn/interrupt", json!({"threadId":id,"turnId":id})),
    ];
    thread["ephemeral"] = json!(false);
    examples.push(("thread/start", thread.clone()));
    thread.as_object_mut().unwrap().remove("ephemeral");
    thread["threadId"] = json!(id);
    examples.push(("thread/resume", thread));
    for (method, params) in examples {
        validate_request(validator, &json!({"id":1,"method":method,"params":params}))?;
    }
    Ok(())
}

fn fields(schema: &Value, expected: &Value) -> Result<(), String> {
    if let Some(fields) = expected.as_array() {
        for field in fields {
            let name = field.as_str().unwrap();
            if schema["properties"].get(name).is_none() {
                return Err(format!("missing published field {name}"));
            }
        }
    }
    Ok(())
}

fn variant<'a>(schema: &'a Value, tag: &str, value: &str) -> Result<&'a Value, String> {
    schema["oneOf"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|item| {
                item["properties"][tag]["enum"]
                    .as_array()
                    .is_some_and(|values| values.contains(&json!(value)))
            })
        })
        .ok_or_else(|| format!("missing published {tag} {value} (unsupported schema layout)"))
}

fn inventory(schema: &Value, expected: &Value) -> Result<(), String> {
    if !schema.is_object() {
        return Err("expected a schema object".into());
    }
    fields(schema, &expected["fields"])?;
    if let Some(signals) = expected["signals"].as_array() {
        for signal in signals {
            variant(schema, "method", signal.as_str().unwrap())?;
        }
    }
    if let Some(methods) = expected["methods"].as_object() {
        for (method, params) in methods {
            let item = variant(schema, "method", method)?;
            let reference = item["properties"]["params"]["$ref"]
                .as_str()
                .and_then(|r| r.strip_prefix('#'))
                .ok_or("missing local parameter schema reference")?;
            let definition = schema
                .pointer(reference)
                .ok_or("missing parameter definition")?;
            fields(definition, params).map_err(|e| format!("{method}: {e}"))?;
        }
    }
    if let Some(definitions) = expected["definitions"].as_object() {
        for (name, expected) in definitions {
            fields(&schema["definitions"][name], expected).map_err(|e| format!("{name}: {e}"))?;
        }
    }
    if let Some(definitions) = expected["variants"].as_object() {
        for (name, variants) in definitions {
            for (tag, expected) in variants.as_object().unwrap() {
                fields(
                    variant(&schema["definitions"][name], "type", tag)?,
                    expected,
                )?;
            }
        }
    }
    Ok(())
}

fn published_fields_only(schema: &mut Value) {
    // JSON Schema normally permits unknown object fields. Team must not send fields the
    // backend does not publish: a silently ignored sandbox override is not compatibility.
    match schema {
        Value::Object(object) => {
            if object.get("type") == Some(&json!("object")) && object.contains_key("properties") {
                object.entry("additionalProperties").or_insert(json!(false));
            }
            for value in object.values_mut() {
                published_fields_only(value);
            }
        }
        Value::Array(items) => {
            for value in items {
                published_fields_only(value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the installed native Codex CLI; no auth or model calls"]
    fn installed_native_schema_supports_team_requests() {
        let root = std::env::temp_dir().join(format!(
            "team-native-schema-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("codex-home")).unwrap();
        let options = options_for_schema(&root);
        let validator = preflight(&options, &root, &root).unwrap();
        for network in [false, true] {
            let mut options = options_for_schema(&root);
            options.execution = crate::profile::Execution::from_context(&json!({
                "approval_policy":"never", "cwd":root,
                "sandbox_policy":{"type":"workspace-write","writable_roots":[],
                    "network_access":network,"exclude_tmpdir_env_var":true,"exclude_slash_tmp":true}
            }))
            .unwrap();
            validate_examples(&validator, &options).unwrap();
        }
        fs::remove_dir_all(&root).unwrap();
    }

    fn options_for_schema(root: &Path) -> Options {
        crate::parse_using(
            &[
                "run",
                "--model",
                "schema-probe",
                "--effort",
                "high",
                "--task",
                "schema only",
                "--completion-gate",
                "schema only",
                "--cwd",
                root.to_str().unwrap(),
                "--store",
                root.to_str().unwrap(),
            ]
            .map(str::to_owned),
            || Ok(json!({"approval_policy":"never","sandbox_policy":{"type":"danger-full-access"},"permission_profile":{"type":"disabled"}})),
        )
        .unwrap()
    }

    #[test]
    fn missing_methods_and_ignored_fields_are_not_capabilities() {
        let mut schema = json!({"oneOf":[{"type":"object","properties":{
            "method":{"enum":["turn/start"]},"params":{"$ref":"#/definitions/Params"}
        }}],"definitions":{"Params":{"type":"object","properties":{"threadId":{"type":"string"}}}}});
        assert!(inventory(&schema, &json!({"methods":{"turn/interrupt":[]}})).is_err());
        assert!(
            inventory(
                &schema,
                &json!({"methods":{"turn/start":["sandboxPolicy"]}})
            )
            .is_err()
        );
        inventory(&schema, &json!({"methods":{"turn/start":["threadId"]}})).unwrap();
        published_fields_only(&mut schema);
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(validator.is_valid(&json!({"method":"turn/start","params":{"threadId":"t"}})));
        assert!(!validator.is_valid(
            &json!({"method":"turn/start","params":{"threadId":"t","sandboxPolicy":{}}})
        ));
        assert!(!validator.is_valid(&json!({"method":"turn/start","params":{"threadId":4}})));
    }
}
