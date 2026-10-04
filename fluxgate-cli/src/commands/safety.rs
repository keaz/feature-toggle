//! Freeze windows, canary gates and stage criteria.

use clap::{Args, Subcommand};
use reqwest::Method;

use super::{App, call, json_data, optional_data};
use crate::error::CliError;
use crate::output::Outcome;

#[derive(Debug, Args)]
pub struct FreezeArgs {
    #[command(subcommand)]
    pub command: FreezeCommand,
}

#[derive(Debug, Subcommand)]
pub enum FreezeCommand {
    /// Freeze windows in effect now (create and edit them with `admin freeze-windows`).
    Active,
}

#[derive(Debug, Args)]
pub struct CanaryArgs {
    #[command(subcommand)]
    pub command: CanaryCommand,
}

#[derive(Debug, Subcommand)]
pub enum CanaryCommand {
    /// Show a stage's canary gates.
    Gates { stage_id: String },
    /// Replace a stage's canary gates (JSON body).
    Set {
        stage_id: String,
        #[arg(long)]
        data: String,
    },
    /// Analyze a canary gate now.
    Analyze {
        gate_id: String,
        #[arg(long)]
        data: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct CriteriaArgs {
    #[command(subcommand)]
    pub command: CriteriaCommand,
}

#[derive(Debug, Subcommand)]
pub enum CriteriaCommand {
    /// Show a stage's targeting criteria.
    Get { stage_id: String },
    /// Replace a stage's targeting criteria (JSON body).
    Set {
        stage_id: String,
        #[arg(long)]
        data: String,
    },
    /// Replace a criterion's variant allocations (JSON body).
    Variants {
        criteria_id: String,
        #[arg(long)]
        data: String,
    },
}

pub async fn freeze(args: FreezeArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    match args.command {
        FreezeCommand::Active => {
            let team = context.team_id().await?;
            call(
                &context,
                Method::GET,
                &["teams", &team, "freeze-windows", "active"],
                &[],
                None,
            )
            .await
        }
    }
}

pub async fn canary(args: CanaryArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let body = match &args.command {
        CanaryCommand::Set { data, .. } => Some(json_data(data)?),
        CanaryCommand::Analyze { data, .. } => Some(optional_data(data.as_deref())?),
        CanaryCommand::Gates { .. } => None,
    };
    let context = app.connect().await?;
    match &args.command {
        CanaryCommand::Gates { stage_id } => {
            call(
                &context,
                Method::GET,
                &["stages", stage_id, "canary-gates"],
                &[],
                None,
            )
            .await
        }
        CanaryCommand::Set { stage_id, .. } => {
            call(
                &context,
                Method::PUT,
                &["stages", stage_id, "canary-gates"],
                &[],
                body,
            )
            .await
        }
        CanaryCommand::Analyze { gate_id, .. } => {
            call(
                &context,
                Method::POST,
                &["canary-gates", gate_id, "analyze"],
                &[],
                body,
            )
            .await
        }
    }
}

pub async fn criteria(args: CriteriaArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let body = match &args.command {
        CriteriaCommand::Set { data, .. } | CriteriaCommand::Variants { data, .. } => {
            Some(json_data(data)?)
        }
        CriteriaCommand::Get { .. } => None,
    };
    let context = app.connect().await?;
    match &args.command {
        CriteriaCommand::Get { stage_id } => {
            call(
                &context,
                Method::GET,
                &["stages", stage_id, "criteria"],
                &[],
                None,
            )
            .await
        }
        CriteriaCommand::Set { stage_id, .. } => {
            call(
                &context,
                Method::PUT,
                &["stages", stage_id, "criteria"],
                &[],
                body,
            )
            .await
        }
        CriteriaCommand::Variants { criteria_id, .. } => {
            call(
                &context,
                Method::PUT,
                &["criteria", criteria_id, "variant-allocations"],
                &[],
                body,
            )
            .await
        }
    }
}
