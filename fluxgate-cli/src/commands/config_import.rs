//! `fluxgate config import`: create the environments and flags of an export
//! that the current team does not have. Nothing existing is changed.

use serde_json::{Map, Value, json};

use super::App;
use crate::error::CliError;
use crate::output::{Kind, Outcome};

/// Flag fields copied from an export into `POST /teams/{team}/features`.
const FEATURE_FIELDS: [&str; 11] = [
    "description",
    "featureType",
    "enabled",
    "lifecycleStage",
    "owner",
    "purpose",
    "referenceUrl",
    "expiresAt",
    "cleanupReason",
    "tags",
    "flagKind",
];

pub async fn run(file: &str, dry_run: bool, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let text = std::fs::read_to_string(file)
        .map_err(|err| CliError::Usage(format!("cannot read {file}: {err}")))?;
    let export: Value = serde_json::from_str(&text)
        .map_err(|err| CliError::Usage(format!("{file} is not a config export: {err}")))?;
    let list = |key: &str| -> Result<Vec<Value>, CliError> {
        export
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "{file} has no {key} list; expected a config export"
                ))
            })
    };
    let environments = list("environments")?;
    let features = list("features")?;

    let context = app.connect().await?;
    let team = context.team_id().await?;
    let existing_envs = context
        .api
        .get_all_pages(&["teams", &team, "environments"], &[])
        .await?;
    // Archived flags still own their keys.
    let existing_features = context
        .api
        .get_all_pages(
            &["teams", &team, "features"],
            &[("includeArchived", "true".to_string())],
        )
        .await?;
    let has = |items: &[Value], field: &str, value: &str| {
        items.iter().any(|item| {
            item.get(field)
                .and_then(Value::as_str)
                .is_some_and(|found| found.eq_ignore_ascii_case(value))
        })
    };

    let mut created_envs = Vec::new();
    let mut skipped_envs = Vec::new();
    for environment in &environments {
        let Some(name) = environment.get("name").and_then(Value::as_str) else {
            continue;
        };
        if has(&existing_envs, "name", name) {
            skipped_envs.push(name.to_string());
            continue;
        }
        let mut body = json!({
            "name": name,
            "active": environment.get("active").and_then(Value::as_bool).unwrap_or(true),
        });
        if let Some(kind) = environment.get("environmentType").filter(|v| !v.is_null()) {
            body["environmentType"] = kind.clone();
        }
        if !dry_run {
            context
                .api
                .post(&["teams", &team, "environments"], &body)
                .await?;
        }
        created_envs.push(name.to_string());
    }

    let mut created_features = Vec::new();
    let mut skipped_features = Vec::new();
    for feature in &features {
        let Some(key) = feature.get("key").and_then(Value::as_str) else {
            continue;
        };
        if has(&existing_features, "key", key) {
            skipped_features.push(key.to_string());
            continue;
        }
        let mut body = Map::new();
        body.insert("key".into(), json!(key));
        for field in FEATURE_FIELDS {
            if let Some(value) = feature.get(field).filter(|v| !v.is_null()) {
                body.insert(field.into(), value.clone());
            }
        }
        // Stages, dependencies and variants point at ids of the source team.
        for field in ["dependencies", "relationships", "stages"] {
            body.insert(field.into(), json!([]));
        }
        if !dry_run {
            context
                .api
                .post(&["teams", &team, "features"], &Value::Object(body))
                .await?;
        }
        created_features.push(key.to_string());
    }

    Ok(Outcome::new(
        json!({
            "dryRun": dry_run,
            "team": team,
            "created": { "environments": created_envs, "features": created_features },
            "skipped": { "environments": skipped_envs, "features": skipped_features },
            "notes": [
                "stages, variants, dependencies and targeting criteria are not imported: they refer to ids of the source team; set them up with flags update, criteria set and canary set"
            ],
        }),
        Kind::Document,
    ))
}
