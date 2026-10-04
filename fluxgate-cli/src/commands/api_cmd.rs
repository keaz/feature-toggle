//! `fluxgate api`: any endpoint, like `gh api`.

use clap::Args;
use reqwest::Method;

use super::{App, borrow_query, optional_data, query_pairs};
use crate::error::CliError;
use crate::output::{Kind, Outcome};

#[derive(Debug, Args)]
pub struct ApiArgs {
    /// HTTP method, such as GET or POST.
    pub method: String,
    /// Path under the API base URL, e.g. /teams/{team}/features. `{team}` and
    /// `{env}` are replaced with the resolved team and environment ids.
    pub path: String,
    /// JSON body (inline, @file or - for stdin).
    #[arg(long)]
    pub data: Option<String>,
    /// Query parameter; repeat for several.
    #[arg(long = "query", value_name = "KEY=VALUE")]
    pub query: Vec<String>,
}

pub async fn run(args: ApiArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let method = Method::from_bytes(args.method.trim().to_ascii_uppercase().as_bytes())
        .map_err(|_| CliError::Usage(format!("invalid HTTP method '{}'", args.method)))?;
    let body = match (&args.data, &method) {
        (Some(data), _) => Some(optional_data(Some(data))?),
        (None, &Method::POST | &Method::PUT | &Method::PATCH) => Some(serde_json::json!({})),
        (None, _) => None,
    };
    let query = query_pairs(&args.query)?;
    let context = app.connect().await?;
    let mut path = args.path.clone();
    if path.contains("{team}") {
        path = path.replace("{team}", &context.team_id().await?);
    }
    if path.contains("{env}") {
        let team = context.team_id().await?;
        path = path.replace("{env}", &context.environment_id(&team).await?);
    }
    let value = context
        .api
        .request_raw(method, &path, &borrow_query(&query), body.as_ref())
        .await?;
    Ok(Outcome::new(value, Kind::Auto))
}
