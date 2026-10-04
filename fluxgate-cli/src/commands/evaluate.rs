use serde_json::{Value, json};

use super::App;
use crate::cli::EvaluateArgs;
use crate::error::{CliError, EXIT_FLAG_OFF, EXIT_OK};
use crate::output::{Kind, Outcome, cell};

pub async fn run(args: EvaluateArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = match serde_json::from_str::<Value>(&args.context) {
        Ok(Value::Object(context)) => context,
        Ok(_) => return Err(CliError::Usage("--context must be a JSON object".into())),
        Err(err) => return Err(CliError::Usage(format!("--context must be JSON: {err}"))),
    };
    let api_context = app.connect().await?;
    let team = api_context.team_id().await?;
    let environment = api_context.environment_id(&team).await?;
    let value = api_context
        .api
        .post(
            &["evaluate"],
            &json!({
                "teamId": team,
                "featureKey": args.flag,
                "environmentId": environment,
                "targetingKey": args.targeting_key,
                "context": context,
            }),
        )
        .await?;
    let mut outcome = Outcome::new(value, Kind::Object);
    if args.exit_code {
        outcome.exit_code = match outcome.value.get("value") {
            Some(Value::Bool(true)) => EXIT_OK,
            Some(Value::Bool(false)) => EXIT_FLAG_OFF,
            other => {
                return Err(CliError::Usage(format!(
                    "--exit-code needs a boolean flag; '{}' returned {}",
                    args.flag,
                    cell(other)
                )));
            }
        };
    }
    Ok(outcome)
}
