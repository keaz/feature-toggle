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
    /// 24h, 7d or 30d (also day, week, month) for summary, rates and features.
    #[arg(long)]
    pub period: Option<String>,
    /// Time window ending now, such as 30m, 24h or 7d (count, by-feature, growth).
    #[arg(long, default_value = "24h")]
    pub since: String,
    /// Minutes per point (rates).
    #[arg(long, default_value_t = 60)]
    pub interval_minutes: i32,
    /// Bucket size (growth).
    #[arg(long, default_value = "day")]
    pub interval: String,
    /// Flag key (needed by experiments and features).
    #[arg(long)]
    pub flag: Option<String>,
    /// Extra query parameter; repeat for several. The team is added as teamId
    /// and the environment as environmentId when configured.
    #[arg(long = "query", value_name = "KEY=VALUE")]
    pub query: Vec<String>,
}

/// A reporting period. The summary and rates reports and the per-flag report
/// name the same periods differently.
#[derive(Debug, Clone, Copy)]
enum Period {
    Day,
    Week,
    Month,
}

impl Period {
    fn parse(value: &str) -> Result<Self, CliError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "24h" | "day" | "period_24h" => Ok(Period::Day),
            "7d" | "week" | "period_7d" => Ok(Period::Week),
            "30d" | "month" | "period_30d" => Ok(Period::Month),
            other => Err(CliError::Usage(format!(
                "--period {other}: expected 24h, 7d or 30d"
            ))),
        }
    }

    fn summary_name(self) -> String {
        match self {
            Period::Day => "PERIOD_24H",
            Period::Week => "PERIOD_7D",
            Period::Month => "PERIOD_30D",
        }
        .to_string()
    }

    fn feature_name(self) -> String {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
        }
        .to_string()
    }
}

/// `30m`, `24h` or `7d`.
fn parse_since(value: &str) -> Result<chrono::Duration, CliError> {
    let value = value.trim();
    let (number, unit) = value.split_at(value.len().saturating_sub(1));
    let amount: i64 = number.parse().map_err(|_| {
        CliError::Usage(format!("--since {value}: expected a number and m, h or d"))
    })?;
    match unit {
        "m" => Ok(chrono::Duration::minutes(amount)),
        "h" => Ok(chrono::Duration::hours(amount)),
        "d" => Ok(chrono::Duration::days(amount)),
        _ => Err(CliError::Usage(format!(
            "--since {value}: expected a number and m, h or d"
        ))),
    }
}

#[derive(Debug, Args)]
pub struct QueryArgs {
    /// Query parameter; repeat for several.
    #[arg(long = "query", value_name = "KEY=VALUE")]
    pub query: Vec<String>,
}

pub async fn metrics(args: MetricsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let extra = query_pairs(&args.query)?;
    let since = parse_since(&args.since)?;
    let needs_flag = matches!(args.report, Report::Experiments | Report::Features);
    if needs_flag && args.flag.is_none() {
        return Err(CliError::Usage("this report needs --flag <key>".into()));
    }
    let context = app.connect().await?;
    let team = optional_team(&context).await?;
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(team) = &team {
        query.push(("teamId", team.clone()));
    }
    if let Some(flag) = &args.flag {
        query.push(("featureKey", flag.clone()));
    }
    let wants_env =
        context.settings.environment.is_some() || matches!(args.report, Report::Features);
    if wants_env && !matches!(args.report, Report::Growth | Report::System) {
        let team = team.clone().ok_or_else(|| {
            CliError::Usage("an environment needs a team: pass --team or set FLUXGATE_TEAM".into())
        })?;
        query.push(("environmentId", context.environment_id(&team).await?));
    }
    let period = Period::parse(args.period.as_deref().unwrap_or("24h"))?;
    let now = chrono::Utc::now();
    let window = |query: &mut Vec<(&str, String)>| {
        query.push(("fromTime", (now - since).to_rfc3339()));
        query.push(("toTime", now.to_rfc3339()));
    };
    match args.report {
        Report::Summary => query.push(("period", period.summary_name())),
        Report::Rates => {
            query.push(("period", period.summary_name()));
            query.push(("intervalMinutes", args.interval_minutes.to_string()));
        }
        Report::Count | Report::ByFeature => window(&mut query),
        Report::Growth => {
            window(&mut query);
            query.push(("interval", args.interval.clone()));
        }
        Report::Features => query.push(("timePeriod", period.feature_name())),
        Report::Experiments | Report::System => {}
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
