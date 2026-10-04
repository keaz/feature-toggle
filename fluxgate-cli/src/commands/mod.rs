//! Command implementations. Each returns an [`Outcome`] for `run` to render.

pub mod accounts;
pub mod admin;
pub mod ai;
pub mod api_cmd;
pub mod approvals;
pub mod config_export;
pub mod config_import;
pub mod configure;
pub mod edge;
pub mod evaluate;
pub mod flags;
pub mod health;
pub mod jira;
pub mod login;
pub mod logout;
pub mod observe;
pub mod rollout;
pub mod safety;
pub mod teams;
pub mod watch;
pub mod whoami;

use serde_json::{Value, json};

use crate::api::ApiClient;
use crate::cli::{Command, PageArgs};
use crate::config::{ConfigFiles, Env, Overrides, Paths, Settings, resolve, selected_profile};
use crate::context::Context;
use crate::error::CliError;
use crate::output::Outcome;
use crate::prompt::Prompter;

pub struct App<'a> {
    pub env: Env,
    pub paths: Paths,
    pub overrides: Overrides,
    pub is_tty: bool,
    pub prompter: &'a mut dyn Prompter,
    /// Standard output, for commands that print while they run (`watch`).
    pub out: &'a mut dyn std::io::Write,
}

impl App<'_> {
    pub fn files(&self) -> Result<ConfigFiles, CliError> {
        ConfigFiles::load(&self.paths)
    }

    pub fn settings(&self, files: &ConfigFiles) -> Result<Settings, CliError> {
        resolve(files, &self.env, &self.overrides, self.is_tty)
    }

    /// The selected profile name, whether or not it exists yet.
    pub fn profile(&self) -> String {
        selected_profile(&self.overrides, &self.env)
    }

    pub async fn connect(&self) -> Result<Context, CliError> {
        let files = self.files()?;
        Context::connect(self.settings(&files)?, self.paths.clone()).await
    }
}

pub async fn dispatch(command: Command, app: &mut App<'_>) -> Result<Outcome, CliError> {
    match command {
        Command::Health => health::run(app).await,
        Command::Flags(args) => flags::run(args, app).await,
        Command::Approvals(args) => approvals::run(args, app).await,
        Command::Evaluate(args) => evaluate::run(args, app).await,
        Command::Config(args) => match args.command {
            crate::cli::ConfigSubcommand::Export => config_export::run(app).await,
            crate::cli::ConfigSubcommand::Import { file, dry_run } => {
                config_import::run(&file, dry_run, app).await
            }
        },
        Command::Rollout(args) => rollout::run(args, app).await,
        Command::Login(args) => login::run(args, app).await,
        Command::Logout(args) => logout::run(args, app).await,
        Command::Configure(args) => configure::run(args, app).await,
        Command::Whoami => whoami::run(app).await,
        Command::Teams(args) => teams::run(args, app).await,
        Command::Api(args) => api_cmd::run(args, app).await,
        Command::Admin(resource) => admin::run(resource, app).await,
        Command::Freeze(args) => safety::freeze(args, app).await,
        Command::Canary(args) => safety::canary(args, app).await,
        Command::Criteria(args) => safety::criteria(args, app).await,
        Command::Jira(args) => jira::run(args, app).await,
        Command::Ai(args) => ai::run(args, app).await,
        Command::Metrics(args) => observe::metrics(args, app).await,
        Command::Audit(args) => observe::audit(args, app).await,
        Command::Activity(args) => observe::activity(args, app).await,
        Command::SystemClients(args) => accounts::system_clients(args, app).await,
        Command::JwtSecrets(args) => accounts::jwt_secrets(args, app).await,
        Command::Sso(args) => accounts::sso(args, app).await,
        Command::Users(args) => accounts::users(args, app).await,
        Command::Notifications(args) => accounts::notifications(args, app).await,
        Command::Completions { shell } => {
            use clap::CommandFactory;
            let mut script = Vec::new();
            clap_complete::generate(
                shell,
                &mut crate::cli::Cli::command(),
                "fluxgate",
                &mut script,
            );
            Ok(Outcome::raw(String::from_utf8_lossy(&script).into_owned()))
        }
        Command::Watch(args) => watch::run(args, app).await,
        Command::Edge(args) => edge::run(args, app).await,
    }
}

/// One page as asked with `--limit`/`--offset`, or every page with `--all`.
pub async fn list_paged(
    api: &ApiClient,
    segments: &[&str],
    query: &[(&str, String)],
    page: &PageArgs,
) -> Result<Value, CliError> {
    if page.all {
        let items = api.get_all_pages(segments, query).await?;
        let total = items.len();
        return Ok(
            json!({ "items": items, "meta": { "offset": 0, "limit": total, "total": total } }),
        );
    }
    let mut page_query = query.to_vec();
    if let Some(limit) = page.limit {
        page_query.push(("limit", limit.to_string()));
    }
    if let Some(offset) = page.offset {
        page_query.push(("offset", offset.to_string()));
    }
    api.get(segments, &page_query).await
}

/// JSON from `--data`: inline JSON, `@file`, or `-` for stdin.
pub fn json_data(raw: &str) -> Result<Value, CliError> {
    let text = if raw == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        text
    } else if let Some(file) = raw.strip_prefix('@') {
        std::fs::read_to_string(file)
            .map_err(|err| CliError::Usage(format!("cannot read --data file {file}: {err}")))?
    } else {
        raw.to_string()
    };
    serde_json::from_str(&text)
        .map_err(|err| CliError::Usage(format!("--data is not valid JSON: {err}")))
}

/// `--data` when given, else an empty object.
pub fn optional_data(raw: Option<&str>) -> Result<Value, CliError> {
    raw.map(json_data)
        .transpose()
        .map(|data| data.unwrap_or_else(|| json!({})))
}

/// `KEY=VALUE` pairs from `--query`.
pub fn query_pairs(raw: &[String]) -> Result<Vec<(String, String)>, CliError> {
    raw.iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(key, value)| (key.trim().to_string(), value.to_string()))
                .filter(|(key, _)| !key.is_empty())
                .ok_or_else(|| CliError::Usage(format!("--query {pair}: expected KEY=VALUE")))
        })
        .collect()
}

/// Borrowed view of owned query pairs, as [`ApiClient`] takes them.
pub fn borrow_query(pairs: &[(String, String)]) -> Vec<(&str, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect()
}

/// The id of a flag given by id or key.
pub async fn feature_id(context: &Context, id_or_key: &str) -> Result<String, CliError> {
    if crate::context::is_uuid(id_or_key) {
        return Ok(id_or_key.trim().to_string());
    }
    let team = context.team_id().await?;
    let feature = context
        .api
        .get(&["teams", &team, "features", "by-key", id_or_key], &[])
        .await?;
    feature
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CliError::Other(format!("flag '{id_or_key}' has no id in the response")))
}

/// The team id when one is configured or implied by the token, else `None`.
pub async fn optional_team(context: &Context) -> Result<Option<String>, CliError> {
    match context.team_id().await {
        Ok(team) => Ok(Some(team)),
        Err(CliError::Usage(message)) if message.starts_with("team required") => Ok(None),
        Err(err) => Err(err),
    }
}

/// Calls an endpoint and shows the result with automatic columns.
pub async fn call(
    context: &Context,
    method: reqwest::Method,
    segments: &[&str],
    query: &[(&str, String)],
    body: Option<Value>,
) -> Result<Outcome, CliError> {
    let value = context
        .api
        .request(method, segments, query, body.as_ref())
        .await?;
    Ok(Outcome::new(value, crate::output::Kind::Auto))
}
