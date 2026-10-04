//! Evaluation through an edge server with OFREP, the path SDKs use.

use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};

use super::App;
use crate::api::ApiClient;
use crate::context::Context;
use crate::error::{CliError, EXIT_FLAG_OFF, EXIT_OK};
use crate::output::{Kind, Outcome, cell};

#[derive(Debug, Args)]
pub struct EdgeArgs {
    /// Edge server address (env FLUXGATE_EDGE_URL, profile key edge_url).
    #[arg(long)]
    pub edge_url: Option<String>,
    #[command(subcommand)]
    pub command: EdgeCommand,
}

#[derive(Debug, Subcommand)]
pub enum EdgeCommand {
    /// Evaluate one flag.
    Evaluate {
        flag: String,
        #[arg(long)]
        targeting_key: String,
        /// Evaluation context as a JSON object.
        #[arg(long, default_value = "{}")]
        context: String,
        /// Exit 0 when the flag is true and 10 when it is false.
        #[arg(long)]
        exit_code: bool,
    },
    /// Evaluate every flag of the SDK key's environment.
    EvaluateAll {
        #[arg(long)]
        targeting_key: String,
        #[arg(long, default_value = "{}")]
        context: String,
    },
}

pub async fn run(args: EdgeArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let files = app.files()?;
    let profile = app.profile();
    let settings = app.settings(&files)?;
    let edge_url = args
        .edge_url
        .clone()
        .or_else(|| app.env.get("FLUXGATE_EDGE_URL").map(str::to_string))
        .or_else(|| files.profile_value(&profile, "edge_url").map(str::to_string))
        .ok_or_else(|| {
            CliError::Usage("edge address required: pass --edge-url, set FLUXGATE_EDGE_URL, or configure set edge_url".into())
        })?;
    // The SDK key (`<clientId>.<apiKey>`) is a secret: env or credentials file.
    let edge_key = app
        .env
        .get("FLUXGATE_EDGE_KEY")
        .map(str::to_string)
        .or_else(|| {
            files
                .credential_value(&profile, "edge_key")
                .map(str::to_string)
        });
    let api = ApiClient::new(&edge_url, edge_key, Context::timeout(&settings))?;
    match args.command {
        EdgeCommand::Evaluate {
            flag,
            targeting_key,
            context,
            exit_code,
        } => {
            let body = json!({ "context": context_with(&targeting_key, &context)? });
            let value = api
                .post(&["ofrep", "v1", "evaluate", "flags", &flag], &body)
                .await?;
            let mut outcome = Outcome::new(value, Kind::Object);
            if exit_code {
                outcome.exit_code = match outcome.value.get("value") {
                    Some(Value::Bool(true)) => EXIT_OK,
                    Some(Value::Bool(false)) => EXIT_FLAG_OFF,
                    other => {
                        return Err(CliError::Usage(format!(
                            "--exit-code needs a boolean flag; '{flag}' returned {}",
                            cell(other)
                        )));
                    }
                };
            }
            Ok(outcome)
        }
        EdgeCommand::EvaluateAll {
            targeting_key,
            context,
        } => {
            let body = json!({ "context": context_with(&targeting_key, &context)? });
            let value = api
                .post(&["ofrep", "v1", "evaluate", "flags"], &body)
                .await?;
            let flags = value.get("flags").cloned().unwrap_or(value);
            Ok(Outcome::new(flags, Kind::Auto))
        }
    }
}

/// The OFREP context: `targetingKey` plus the attributes of `--context`.
fn context_with(targeting_key: &str, raw: &str) -> Result<Map<String, Value>, CliError> {
    let mut context = match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(context)) => context,
        Ok(_) => return Err(CliError::Usage("--context must be a JSON object".into())),
        Err(err) => return Err(CliError::Usage(format!("--context must be JSON: {err}"))),
    };
    context.insert("targetingKey".into(), json!(targeting_key));
    Ok(context)
}
