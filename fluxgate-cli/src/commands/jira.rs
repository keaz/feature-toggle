//! Jira integration operations (create and edit integrations with
//! `admin jira-integrations`).

use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::json;

use super::{App, call, json_data};
use crate::error::CliError;
use crate::output::Outcome;

#[derive(Debug, Args)]
pub struct JiraArgs {
    #[command(subcommand)]
    pub command: JiraCommand,
}

#[derive(Debug, Subcommand)]
pub enum JiraCommand {
    /// Show an integration's status rules.
    Rules { integration_id: String },
    /// Replace an integration's status rules (JSON body).
    SetRules {
        integration_id: String,
        #[arg(long)]
        data: String,
    },
    /// Rotate the inbound secret (shown once).
    RotateSecret { integration_id: String },
    /// Create, or with --delete remove, the native webhook secret.
    WebhookSecret {
        integration_id: String,
        #[arg(long)]
        delete: bool,
    },
    /// Configure write-back to Jira (JSON body).
    Writeback {
        integration_id: String,
        #[arg(long)]
        data: String,
    },
    /// Test the write-back connection.
    WritebackTest { integration_id: String },
    /// Resume paused write-back.
    WritebackResume { integration_id: String },
    /// Recent inbound Jira events.
    Events { integration_id: String },
    /// Outbound write-back jobs.
    Jobs { integration_id: String },
    /// Retry a dead outbound job.
    Retry {
        integration_id: String,
        job_id: String,
    },
}

pub async fn run(args: JiraArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let data = match &args.command {
        JiraCommand::SetRules { data, .. } | JiraCommand::Writeback { data, .. } => {
            Some(json_data(data)?)
        }
        _ => None,
    };
    let context = app.connect().await?;
    let empty = || Some(json!({}));
    let base = "jira-integrations";
    match &args.command {
        JiraCommand::Rules { integration_id } => {
            call(
                &context,
                Method::GET,
                &[base, integration_id, "rules"],
                &[],
                None,
            )
            .await
        }
        JiraCommand::SetRules { integration_id, .. } => {
            call(
                &context,
                Method::PUT,
                &[base, integration_id, "rules"],
                &[],
                data,
            )
            .await
        }
        JiraCommand::RotateSecret { integration_id } => {
            call(
                &context,
                Method::POST,
                &[base, integration_id, "rotate-secret"],
                &[],
                empty(),
            )
            .await
        }
        JiraCommand::WebhookSecret {
            integration_id,
            delete: false,
        } => {
            call(
                &context,
                Method::POST,
                &[base, integration_id, "native-webhook-secret"],
                &[],
                empty(),
            )
            .await
        }
        JiraCommand::WebhookSecret {
            integration_id,
            delete: true,
        } => {
            call(
                &context,
                Method::DELETE,
                &[base, integration_id, "native-webhook-secret"],
                &[],
                None,
            )
            .await
        }
        JiraCommand::Writeback { integration_id, .. } => {
            call(
                &context,
                Method::PUT,
                &[base, integration_id, "writeback"],
                &[],
                data,
            )
            .await
        }
        JiraCommand::WritebackTest { integration_id } => {
            call(
                &context,
                Method::POST,
                &[base, integration_id, "writeback", "test"],
                &[],
                empty(),
            )
            .await
        }
        JiraCommand::WritebackResume { integration_id } => {
            call(
                &context,
                Method::POST,
                &[base, integration_id, "writeback", "resume"],
                &[],
                empty(),
            )
            .await
        }
        JiraCommand::Events { integration_id } => {
            call(
                &context,
                Method::GET,
                &[base, integration_id, "events"],
                &[],
                None,
            )
            .await
        }
        JiraCommand::Jobs { integration_id } => {
            call(
                &context,
                Method::GET,
                &[base, integration_id, "outbound-jobs"],
                &[],
                None,
            )
            .await
        }
        JiraCommand::Retry {
            integration_id,
            job_id,
        } => {
            call(
                &context,
                Method::POST,
                &[base, integration_id, "outbound-jobs", job_id, "retry"],
                &[],
                empty(),
            )
            .await
        }
    }
}
