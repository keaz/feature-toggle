//! AI features: status, team settings, justification check, suggestions.

use clap::{Args, Subcommand};
use reqwest::Method;

use super::{App, call, json_data, optional_data};
use crate::error::CliError;
use crate::output::Outcome;

#[derive(Debug, Args)]
pub struct AiArgs {
    #[command(subcommand)]
    pub command: AiCommand,
}

#[derive(Debug, Subcommand)]
pub enum AiCommand {
    /// Whether AI features are available.
    Status,
    /// Show the team's AI settings.
    Settings,
    /// Replace the team's AI settings (JSON body).
    SetSettings {
        #[arg(long)]
        data: String,
    },
    /// Check a change justification (JSON body).
    Justify {
        #[arg(long)]
        data: String,
    },
    /// Suggest flag settings from a description (JSON body).
    Suggest {
        #[arg(long)]
        data: String,
    },
    /// Classify flags without a kind.
    BackfillKinds {
        #[arg(long)]
        data: Option<String>,
    },
}

pub async fn run(args: AiArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let data = match &args.command {
        AiCommand::SetSettings { data }
        | AiCommand::Justify { data }
        | AiCommand::Suggest { data } => Some(json_data(data)?),
        AiCommand::BackfillKinds { data } => Some(optional_data(data.as_deref())?),
        _ => None,
    };
    let context = app.connect().await?;
    if matches!(args.command, AiCommand::Status) {
        return call(&context, Method::GET, &["ai", "status"], &[], None).await;
    }
    let team = context.team_id().await?;
    let (method, tail): (Method, &[&str]) = match &args.command {
        AiCommand::Status => unreachable!("handled above"),
        AiCommand::Settings => (Method::GET, &["ai-settings"]),
        AiCommand::SetSettings { .. } => (Method::PUT, &["ai-settings"]),
        AiCommand::Justify { .. } => (Method::POST, &["ai", "justification-check"]),
        AiCommand::Suggest { .. } => (Method::POST, &["ai", "feature-suggestions"]),
        AiCommand::BackfillKinds { .. } => (Method::POST, &["ai", "flag-kind", "backfill"]),
    };
    let mut segments = vec!["teams", team.as_str()];
    segments.extend_from_slice(tail);
    call(&context, method, &segments, &[], data).await
}
