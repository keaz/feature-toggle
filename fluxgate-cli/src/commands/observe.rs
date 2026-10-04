//! Metrics, audit analytics and the activity feed.

use clap::Args;
use reqwest::Method;

use super::{App, call, optional_team, query_pairs};
use crate::error::CliError;
use crate::output::Outcome;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Report {
    /// Evaluation totals.
    Summary,
    /// Evaluation counts.
    Count,
    /// Evaluation rates over time.
    Rates,
    /// Evaluations per flag.
    ByFeature,
    /// System-wide evaluation metrics.
    System,
    /// Experiment results.
    Experiments,
    /// Flag growth over time.
    Growth,
    /// Metrics per flag.
    Features,
}

impl Report {
    fn segments(self) -> &'static [&'static str] {
        match self {
            Report::Summary => &["metrics", "evaluations", "summary"],
            Report::Count => &["metrics", "evaluations", "count"],
            Report::Rates => &["metrics", "evaluations", "rates"],
            Report::ByFeature => &["metrics", "evaluations", "by-feature"],
            Report::System => &["metrics", "evaluations", "system"],
            Report::Experiments => &["metrics", "experiment-results"],
            Report::Growth => &["metrics", "feature-growth"],
            Report::Features => &["metrics", "by-feature"],
        }
    }
}

#[derive(Debug, Args)]
pub struct MetricsArgs {
    #[arg(value_enum)]
    pub report: Report,
    /// Query parameter such as period=day; repeat for several. The team is
    /// added as teamId when one is configured.
    #[arg(long = "query", value_name = "KEY=VALUE")]
    pub query: Vec<String>,
}

#[derive(Debug, Args)]
pub struct QueryArgs {
    /// Query parameter; repeat for several.
    #[arg(long = "query", value_name = "KEY=VALUE")]
    pub query: Vec<String>,
}

pub async fn metrics(args: MetricsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let extra = query_pairs(&args.query)?;
    let context = app.connect().await?;
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(team) = optional_team(&context).await? {
        query.push(("teamId", team));
    }
    query.extend(
        extra
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone())),
    );
    call(&context, Method::GET, args.report.segments(), &query, None).await
}

pub async fn audit(args: QueryArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let query = query_pairs(&args.query)?;
    let context = app.connect().await?;
    let team = context.team_id().await?;
    let query: Vec<(&str, String)> = query
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect();
    call(
        &context,
        Method::GET,
        &["teams", &team, "audit-analytics"],
        &query,
        None,
    )
    .await
}

pub async fn activity(args: QueryArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let query = query_pairs(&args.query)?;
    let context = app.connect().await?;
    let query: Vec<(&str, String)> = query
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect();
    call(&context, Method::GET, &["activity", "recent"], &query, None).await
}
