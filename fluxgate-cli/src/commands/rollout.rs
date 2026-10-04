use serde_json::json;

use super::App;
use crate::cli::{RolloutArgs, RolloutSubcommand};
use crate::error::CliError;
use crate::output::{Kind, Outcome};

pub async fn run(args: RolloutArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let RolloutSubcommand::Promote {
        stage_id,
        flag,
        request,
        reason,
        external_ref,
        freeze_override_reason,
    } = args.command;
    if stage_id.is_none() && flag.is_none() {
        return Err(CliError::Usage(
            "pass a stage id, or --flag <key> with --env <name>".into(),
        ));
    }
    let mut body = json!({ "request": request.to_ascii_uppercase() });
    if let Some(reason) = reason {
        body["reason"] = json!(reason);
    }
    if let Some(external_ref) = external_ref {
        body["externalRef"] = json!(external_ref);
    }
    if let Some(freeze_override_reason) = freeze_override_reason {
        body["freezeOverrideReason"] = json!(freeze_override_reason);
    }

    let context = app.connect().await?;
    let value = match (stage_id, flag) {
        (_, Some(flag)) => {
            let team = context.team_id().await?;
            // The by-key route addresses the environment by name.
            let environment = context.environment_name(&team).await?;
            context
                .api
                .post(
                    &[
                        "teams",
                        &team,
                        "features",
                        "by-key",
                        &flag,
                        "environments",
                        &environment,
                        "request-change",
                    ],
                    &body,
                )
                .await?
        }
        (Some(stage_id), None) => {
            context
                .api
                .post(&["stages", &stage_id, "request-change"], &body)
                .await?
        }
        (None, None) => unreachable!("checked above"),
    };
    Ok(Outcome::new(value, Kind::Object))
}
