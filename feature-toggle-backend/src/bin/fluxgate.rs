use clap::{Args, Parser, Subcommand};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(name = "fluxgate")]
#[command(about = "FluxGate CLI for flag operations and CI automation")]
struct Cli {
    #[arg(
        long,
        env = "FLUXGATE_URL",
        default_value = "http://localhost:8080/api/v1"
    )]
    base_url: String,
    #[arg(long, env = "FLUXGATE_TOKEN")]
    token: Option<String>,
    #[arg(long, env = "FLUXGATE_TEAM_ID")]
    team_id: Option<String>,
    #[arg(long, default_value_t = false)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Health,
    Flags(FlagsCommand),
    Evaluate(EvaluateCommand),
    Approvals(ApprovalsCommand),
    Config(ConfigCommand),
    Rollout(RolloutCommand),
}

#[derive(Debug, Args)]
struct FlagsCommand {
    #[command(subcommand)]
    command: FlagsSubcommand,
}

#[derive(Debug, Subcommand)]
enum FlagsSubcommand {
    List {
        #[arg(long)]
        team_id: Option<String>,
    },
    Get {
        id: String,
    },
}

#[derive(Debug, Args)]
struct EvaluateCommand {
    #[arg(long)]
    feature_key: String,
    #[arg(long, env = "FLUXGATE_ENVIRONMENT_ID")]
    environment_id: String,
    #[arg(long)]
    targeting_key: String,
    #[arg(long)]
    team_id: Option<String>,
    #[arg(long, default_value = "{}")]
    context: String,
}

#[derive(Debug, Args)]
struct ApprovalsCommand {
    #[command(subcommand)]
    command: ApprovalsSubcommand,
}

#[derive(Debug, Subcommand)]
enum ApprovalsSubcommand {
    List {
        #[arg(long)]
        team_id: Option<String>,
        #[arg(long, default_value = "pending")]
        status: String,
    },
}

#[derive(Debug, Args)]
struct ConfigCommand {
    #[command(subcommand)]
    command: ConfigSubcommand,
}

#[derive(Debug, Subcommand)]
enum ConfigSubcommand {
    Export {
        #[arg(long)]
        team_id: Option<String>,
    },
}

#[derive(Debug, Args)]
struct RolloutCommand {
    #[command(subcommand)]
    command: RolloutSubcommand,
}

#[derive(Debug, Subcommand)]
enum RolloutSubcommand {
    Promote {
        stage_id: String,
        #[arg(long, default_value = "DEPLOYMENT_REQUESTED")]
        request: String,
    },
}

fn normalize_base_url(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn resolve_team_id(cli: &Cli, override_team_id: Option<String>) -> Result<String, String> {
    override_team_id
        .or_else(|| cli.team_id.clone())
        .ok_or_else(|| "team id required: pass --team-id or set FLUXGATE_TEAM_ID".to_string())
}

fn build_headers(token: Option<&str>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    if let Some(token) = token.filter(|value| !value.trim().is_empty()) {
        let value = HeaderValue::from_str(&format!("Bearer {}", token.trim()))
            .map_err(|err| format!("invalid token header: {err}"))?;
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

async fn get_json(
    client: &reqwest::Client,
    url: String,
    token: Option<&str>,
) -> Result<Value, String> {
    let response = client
        .get(url)
        .headers(build_headers(token)?)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    response_json(response).await
}

async fn post_json(
    client: &reqwest::Client,
    url: String,
    token: Option<&str>,
    body: Value,
) -> Result<Value, String> {
    let response = client
        .post(url)
        .headers(build_headers(token)?)
        .json(&body)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    response_json(response).await
}

async fn response_json(response: reqwest::Response) -> Result<Value, String> {
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    let body = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text)
            .map_err(|err| format!("invalid JSON response: {err}: {text}"))?
    };
    if !status.is_success() {
        return Err(format!("request failed with status {status}: {body}"));
    }
    Ok(body)
}

fn print_output(value: &Value, force_json: bool) {
    if force_json {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        );
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let client = reqwest::Client::new();
    let base = normalize_base_url(&cli.base_url);
    let token = cli.token.as_deref();

    let result = match &cli.command {
        Command::Health => get_json(&client, format!("{base}/health"), token).await,
        Command::Flags(command) => match &command.command {
            FlagsSubcommand::List { team_id } => {
                let team_id = resolve_team_id(&cli, team_id.clone());
                match team_id {
                    Ok(team_id) => {
                        get_json(&client, format!("{base}/teams/{team_id}/features"), token).await
                    }
                    Err(err) => Err(err),
                }
            }
            FlagsSubcommand::Get { id } => {
                get_json(&client, format!("{base}/features/{id}"), token).await
            }
        },
        Command::Evaluate(command) => {
            let context = serde_json::from_str::<Value>(&command.context)
                .map_err(|err| format!("context must be JSON: {err}"));
            match context {
                Ok(Value::Object(context)) => {
                    post_json(
                        &client,
                        format!("{base}/evaluate"),
                        token,
                        json!({
                            "teamId": command.team_id.clone().or_else(|| cli.team_id.clone()),
                            "featureKey": command.feature_key,
                            "environmentId": command.environment_id,
                            "targetingKey": command.targeting_key,
                            "context": context,
                        }),
                    )
                    .await
                }
                Ok(_) => Err("context must be a JSON object".to_string()),
                Err(err) => Err(err),
            }
        }
        Command::Approvals(command) => match &command.command {
            ApprovalsSubcommand::List { team_id, status } => {
                let team_id = resolve_team_id(&cli, team_id.clone());
                match team_id {
                    Ok(team_id) => {
                        get_json(
                            &client,
                            format!("{base}/teams/{team_id}/approval-requests?status={status}"),
                            token,
                        )
                        .await
                    }
                    Err(err) => Err(err),
                }
            }
        },
        Command::Config(command) => match &command.command {
            ConfigSubcommand::Export { team_id } => {
                let team_id = resolve_team_id(&cli, team_id.clone());
                match team_id {
                    Ok(team_id) => {
                        let status =
                            get_json(&client, format!("{base}/developer/ofrep-status"), token)
                                .await;
                        let features = get_json(
                            &client,
                            format!("{base}/teams/{team_id}/features?limit=200"),
                            token,
                        )
                        .await;
                        match (status, features) {
                            (Ok(status), Ok(features)) => Ok(
                                json!({ "teamId": team_id, "status": status, "features": features }),
                            ),
                            (Err(err), _) | (_, Err(err)) => Err(err),
                        }
                    }
                    Err(err) => Err(err),
                }
            }
        },
        Command::Rollout(command) => match &command.command {
            RolloutSubcommand::Promote { stage_id, request } => {
                post_json(
                    &client,
                    format!("{base}/stages/{stage_id}/request-change"),
                    token,
                    json!({ "request": request }),
                )
                .await
            }
        },
    };

    match result {
        Ok(value) => print_output(&value, cli.json),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_evaluate_command_for_ci() {
        let cli = Cli::parse_from([
            "fluxgate",
            "--base-url",
            "http://localhost:8080/api/v1",
            "--token",
            "secret",
            "evaluate",
            "--feature-key",
            "checkout",
            "--environment-id",
            "env",
            "--targeting-key",
            "user-1",
            "--context",
            "{\"plan\":\"pro\"}",
        ]);
        match cli.command {
            Command::Evaluate(command) => {
                assert_eq!(command.feature_key, "checkout");
                assert_eq!(command.environment_id, "env");
                assert_eq!(command.targeting_key, "user-1");
            }
            _ => panic!("expected evaluate command"),
        }
    }

    #[test]
    fn team_id_uses_cli_or_env_value() {
        let cli = Cli::parse_from(["fluxgate", "--team-id", "team-a", "health"]);
        assert_eq!(resolve_team_id(&cli, None).unwrap(), "team-a");
        assert_eq!(
            resolve_team_id(&cli, Some("team-b".to_string())).unwrap(),
            "team-b"
        );
    }

    #[test]
    fn parses_rollout_promote_command() {
        let cli = Cli::parse_from([
            "fluxgate",
            "rollout",
            "promote",
            "stage-123",
            "--request",
            "DEPLOYED",
        ]);
        match cli.command {
            Command::Rollout(command) => match command.command {
                RolloutSubcommand::Promote { stage_id, request } => {
                    assert_eq!(stage_id, "stage-123");
                    assert_eq!(request, "DEPLOYED");
                }
            },
            _ => panic!("expected rollout command"),
        }
    }
}
