//! Create, read, update and delete team and system resources with JSON bodies.

use clap::{Args, Subcommand};
use reqwest::Method;

use super::{App, borrow_query, call, json_data, list_paged, query_pairs};
use crate::cli::PageArgs;
use crate::error::CliError;
use crate::output::{Kind, Outcome};

#[derive(Debug, Subcommand)]
pub enum AdminResource {
    /// Team environments.
    Environments(ResourceArgs),
    /// Team evaluation contexts.
    Contexts(ResourceArgs),
    /// Team SDK clients.
    Clients(ResourceArgs),
    /// Team system clients (CI tokens are under `system-clients`).
    SystemClients(ResourceArgs),
    /// Team pipelines.
    Pipelines(ResourceArgs),
    /// Team rollout templates.
    RolloutTemplates(ResourceArgs),
    /// Team metric definitions.
    MetricDefinitions(ResourceArgs),
    /// Teams.
    Teams(ResourceArgs),
    /// Users.
    Users(ResourceArgs),
    /// Roles.
    Roles(ResourceArgs),
    /// SSO providers.
    SsoProviders(ResourceArgs),
    /// Team Jira integrations.
    JiraIntegrations(ResourceArgs),
    /// Team approval policies.
    ApprovalPolicies(ResourceArgs),
    /// Team freeze windows.
    FreezeWindows(ResourceArgs),
    /// Criteria rule groups.
    RuleGroups(ResourceArgs),
}

#[derive(Debug, Args)]
pub struct ResourceArgs {
    #[command(subcommand)]
    pub action: ResourceAction,
}

#[derive(Debug, Subcommand)]
pub enum ResourceAction {
    /// List them.
    List {
        #[command(flatten)]
        page: PageArgs,
        /// Extra query parameter; repeat for several.
        #[arg(long = "query", value_name = "KEY=VALUE")]
        query: Vec<String>,
    },
    /// Show one.
    Get { id: String },
    /// Create one from JSON (inline, @file or - for stdin).
    Create {
        #[arg(long)]
        data: String,
    },
    /// Change one with a JSON body.
    Update {
        id: String,
        #[arg(long)]
        data: String,
    },
    /// Delete one.
    Delete { id: String },
}

/// Where a resource lives and what the API supports for it.
struct Spec {
    name: &'static str,
    /// Collection under `/teams/{team}/` when true, else at the root.
    team: bool,
    collection: &'static str,
    list: bool,
    create: bool,
    /// Item routes `/{item}/{id}`.
    item: &'static str,
    get: bool,
    update: Option<Method>,
    delete: bool,
}

/// `ops` lists what the API supports: l(ist), c(reate), g(et), u(pdate), d(elete).
fn spec(
    name: &'static str,
    team: bool,
    collection: &'static str,
    item: &'static str,
    ops: &str,
) -> Spec {
    Spec {
        name,
        team,
        collection,
        list: ops.contains('l'),
        create: ops.contains('c'),
        item,
        get: ops.contains('g'),
        update: ops.contains('u').then_some(Method::PATCH),
        delete: ops.contains('d'),
    }
}

impl AdminResource {
    fn parts(&self) -> (Spec, &ResourceArgs) {
        use AdminResource::*;
        match self {
            Environments(a) => (
                spec(
                    "environments",
                    true,
                    "environments",
                    "environments",
                    "lcgud",
                ),
                a,
            ),
            Contexts(a) => (spec("contexts", true, "contexts", "contexts", "lcgud"), a),
            Clients(a) => (spec("clients", true, "clients", "clients", "lcgu"), a),
            SystemClients(a) => (
                spec(
                    "system-clients",
                    true,
                    "system-clients",
                    "system-clients",
                    "lcgu",
                ),
                a,
            ),
            Pipelines(a) => (spec("pipelines", true, "pipelines", "pipelines", "lcgu"), a),
            RolloutTemplates(a) => (
                spec("rollout-templates", true, "rollout-templates", "", "lc"),
                a,
            ),
            MetricDefinitions(a) => (spec("metric-definitions", true, "metrics", "", "lc"), a),
            Teams(a) => (spec("teams", false, "teams", "teams", "lcu"), a),
            Users(a) => (spec("users", false, "users", "users", "lcgu"), a),
            Roles(a) => (spec("roles", false, "roles", "roles", "lcd"), a),
            SsoProviders(a) => (
                spec(
                    "sso-providers",
                    false,
                    "sso/providers",
                    "sso/providers",
                    "lcgud",
                ),
                a,
            ),
            JiraIntegrations(a) => (
                spec(
                    "jira-integrations",
                    true,
                    "jira-integrations",
                    "jira-integrations",
                    "lcgud",
                ),
                a,
            ),
            ApprovalPolicies(a) => (
                spec(
                    "approval-policies",
                    true,
                    "approval-policies",
                    "approval-policies",
                    "lcgud",
                ),
                a,
            ),
            FreezeWindows(a) => (
                spec(
                    "freeze-windows",
                    true,
                    "freeze-windows",
                    "freeze-windows",
                    "lcud",
                ),
                a,
            ),
            RuleGroups(a) => (
                spec("rule-groups", false, "rule-groups", "rule-groups", "cud"),
                a,
            ),
        }
    }
}

pub async fn run(resource: AdminResource, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let (spec, args) = resource.parts();
    let unsupported =
        |action: &str| CliError::Usage(format!("{} does not support {action}", spec.name));
    let allowed = match &args.action {
        ResourceAction::List { .. } => spec.list,
        ResourceAction::Get { .. } => spec.get,
        ResourceAction::Create { .. } => spec.create,
        ResourceAction::Update { .. } => spec.update.is_some(),
        ResourceAction::Delete { .. } => spec.delete,
    };
    if !allowed {
        let action = match &args.action {
            ResourceAction::List { .. } => "list",
            ResourceAction::Get { .. } => "get",
            ResourceAction::Create { .. } => "create",
            ResourceAction::Update { .. } => "update",
            ResourceAction::Delete { .. } => "delete",
        };
        return Err(unsupported(action));
    }
    let body = match &args.action {
        ResourceAction::Create { data } | ResourceAction::Update { data, .. } => {
            Some(json_data(data)?)
        }
        _ => None,
    };
    let query = match &args.action {
        ResourceAction::List { query, .. } => query_pairs(query)?,
        _ => Vec::new(),
    };

    let context = app.connect().await?;
    let team = if spec.team
        && matches!(
            args.action,
            ResourceAction::List { .. } | ResourceAction::Create { .. }
        ) {
        Some(context.team_id().await?)
    } else {
        None
    };
    let mut collection: Vec<&str> = Vec::new();
    if let Some(team) = &team {
        collection.extend(["teams", team.as_str()]);
    }
    collection.extend(spec.collection.split('/'));
    let item = |id: &str| -> Vec<String> {
        spec.item
            .split('/')
            .map(str::to_string)
            .chain([id.to_string()])
            .collect()
    };
    match &args.action {
        ResourceAction::List { page, .. } => {
            let value = list_paged(&context.api, &collection, &borrow_query(&query), page).await?;
            Ok(Outcome::new(value, Kind::Auto))
        }
        ResourceAction::Create { .. } => call(&context, Method::POST, &collection, &[], body).await,
        ResourceAction::Get { id } => {
            let segments = item(id);
            let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
            call(&context, Method::GET, &segments, &[], None).await
        }
        ResourceAction::Update { id, .. } => {
            let segments = item(id);
            let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
            let method = spec.update.clone().expect("checked above");
            call(&context, method, &segments, &[], body).await
        }
        ResourceAction::Delete { id } => {
            let segments = item(id);
            let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
            call(&context, Method::DELETE, &segments, &[], None).await
        }
    }
}
