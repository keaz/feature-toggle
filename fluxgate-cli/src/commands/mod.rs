//! Command implementations. Each returns an [`Outcome`] for `run` to render.

pub mod approvals;
pub mod config_export;
pub mod configure;
pub mod evaluate;
pub mod flags;
pub mod health;
pub mod login;
pub mod logout;
pub mod rollout;

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
        Command::Config(_) => config_export::run(app).await,
        Command::Rollout(args) => rollout::run(args, app).await,
        Command::Login(args) => login::run(args, app).await,
        Command::Logout(args) => logout::run(args, app).await,
        Command::Configure(args) => configure::run(args, app).await,
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
        return Ok(json!({ "items": items, "meta": { "offset": 0, "limit": total, "total": total } }));
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
