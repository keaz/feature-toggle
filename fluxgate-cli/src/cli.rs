//! Command line definitions.

use clap::{Args, Parser, Subcommand};

use crate::config::Overrides;
use crate::output::OutputFormat;

#[derive(Parser)]
#[command(
    name = "fluxgate",
    version,
    about = "FluxGate CLI for flag operations and CI automation"
)]
pub struct Cli {
    /// Profile from ~/.fluxgate/config (env FLUXGATE_PROFILE).
    #[arg(long, global = true)]
    pub profile: Option<String>,
    /// Backend API URL such as https://fluxgate.example.com/api/v1 (env FLUXGATE_URL).
    #[arg(long, alias = "base-url", global = true)]
    pub url: Option<String>,
    /// Team name or id (env FLUXGATE_TEAM).
    #[arg(long, alias = "team-id", global = true)]
    pub team: Option<String>,
    /// Environment name or id (env FLUXGATE_ENVIRONMENT).
    #[arg(long = "env", alias = "environment-id", global = true)]
    pub environment: Option<String>,
    /// Output format (env FLUXGATE_OUTPUT); table on a terminal, json otherwise.
    #[arg(long, value_enum, global = true)]
    pub output: Option<OutputFormat>,
    /// Same as --output json.
    #[arg(long, global = true, conflicts_with = "output")]
    pub json: bool,
    /// Bearer token; wins over profiles and sessions (env FLUXGATE_TOKEN).
    #[arg(long, global = true)]
    pub token: Option<String>,
    /// Request timeout in seconds (env FLUXGATE_TIMEOUT, default 30).
    #[arg(long, global = true)]
    pub timeout: Option<u64>,
    #[command(subcommand)]
    pub command: Command,
}

impl std::fmt::Debug for Cli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cli")
            .field("overrides", &self.overrides())
            .field("json", &self.json)
            .field("command", &self.command)
            .finish()
    }
}

impl Cli {
    pub fn overrides(&self) -> Overrides {
        Overrides {
            profile: self.profile.clone(),
            url: self.url.clone(),
            team: self.team.clone(),
            environment: self.environment.clone(),
            output: if self.json {
                Some(OutputFormat::Json)
            } else {
                self.output
            },
            token: self.token.clone(),
            timeout: self.timeout,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check that the backend is up.
    Health,
    /// Read flags.
    Flags(FlagsArgs),
    /// Approval requests.
    Approvals(ApprovalsArgs),
    /// Evaluate a flag for a targeting key.
    Evaluate(EvaluateArgs),
    /// Team configuration.
    Config(ConfigArgs),
    /// Stage changes.
    Rollout(RolloutArgs),
    /// Log in and cache a session for the profile.
    Login(LoginArgs),
    /// End the profile's session, or every session with --all.
    Logout(LogoutArgs),
    /// Set up a profile, or read and change its settings.
    Configure(ConfigureArgs),
    /// Show who the current credentials belong to.
    Whoami,
    /// Teams you can use.
    Teams(TeamsArgs),
    /// Call any API endpoint.
    Api(crate::commands::api_cmd::ApiArgs),
    /// Create, read, update and delete resources.
    #[command(subcommand)]
    Admin(crate::commands::admin::AdminResource),
    /// Freeze windows.
    Freeze(crate::commands::safety::FreezeArgs),
    /// Canary gates.
    Canary(crate::commands::safety::CanaryArgs),
    /// Stage targeting criteria.
    Criteria(crate::commands::safety::CriteriaArgs),
    /// Jira integrations.
    Jira(crate::commands::jira::JiraArgs),
    /// AI features.
    Ai(crate::commands::ai::AiArgs),
    /// Evaluation and experiment metrics.
    Metrics(crate::commands::observe::MetricsArgs),
    /// Audit analytics.
    Audit(crate::commands::observe::QueryArgs),
    /// Recent activity.
    Activity(crate::commands::observe::QueryArgs),
    /// System-client tokens.
    SystemClients(crate::commands::accounts::SystemClientsArgs),
    /// JWT signing secrets.
    JwtSecrets(crate::commands::accounts::JwtSecretsArgs),
    /// SSO mappings and settings.
    Sso(crate::commands::accounts::SsoArgs),
    /// User roles, teams and passwords.
    Users(crate::commands::accounts::UsersArgs),
    /// Notification settings.
    Notifications(crate::commands::accounts::NotificationsArgs),
}

#[derive(Debug, Clone, Default, Args)]
pub struct PageArgs {
    /// Page size (server maximum 200).
    #[arg(long)]
    pub limit: Option<i64>,
    /// Number of items to skip.
    #[arg(long)]
    pub offset: Option<i64>,
    /// Fetch every page.
    #[arg(long, conflicts_with_all = ["limit", "offset"])]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct FlagsArgs {
    #[command(subcommand)]
    pub command: FlagsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum FlagsSubcommand {
    /// List the team's flags.
    List {
        #[command(flatten)]
        page: PageArgs,
        #[command(flatten)]
        filters: FlagFilters,
    },
    /// Show one flag by id or key.
    Get {
        /// Feature id (UUID) or key.
        id_or_key: String,
    },
    /// Create a flag from JSON (inline, @file or - for stdin).
    Create {
        #[arg(long)]
        data: String,
    },
    /// Change a flag with a JSON body.
    Update {
        id_or_key: String,
        #[arg(long)]
        data: String,
    },
    /// Archive flags (asks for --yes).
    Archive {
        #[arg(required = true)]
        flags: Vec<String>,
        /// Confirm archiving.
        #[arg(long)]
        yes: bool,
    },
    /// Run a bulk action on flags.
    Bulk {
        #[arg(value_enum)]
        action: BulkAction,
        #[arg(required = true)]
        flags: Vec<String>,
        /// New owner (update-owner).
        #[arg(long)]
        owner: Option<String>,
        /// Tags (update-tags); repeat for several.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// New lifecycle stage (update-lifecycle).
        #[arg(long)]
        lifecycle_stage: Option<String>,
        /// Confirm archiving (archive).
        #[arg(long)]
        yes: bool,
    },
    /// List a flag's versions.
    Versions { id_or_key: String },
    /// Show what changed in a version.
    Diff {
        id_or_key: String,
        version_id: String,
    },
    /// Roll a flag back to a version.
    Rollback {
        id_or_key: String,
        version_id: String,
        /// Confirm when the rollback archives the flag.
        #[arg(long)]
        yes: bool,
    },
    /// Show flags that depend on this one.
    Impact { id_or_key: String },
    /// Preview the impact of a change (JSON body).
    ImpactPreview {
        id_or_key: String,
        #[arg(long)]
        data: String,
    },
    /// Emergency-disable a flag (kill switch).
    Kill {
        id_or_key: String,
        #[arg(long)]
        reason: String,
        /// Re-enable automatically after this many minutes.
        #[arg(long, value_name = "MINUTES")]
        rollback_in: Option<i32>,
        /// When the override expires (RFC 3339).
        #[arg(long)]
        expires_at: Option<String>,
    },
    /// Lift an emergency disable.
    Unkill {
        id_or_key: String,
        #[arg(long)]
        reason: String,
    },
    /// List active kill switches.
    KillSwitches,
    /// List a flag's scheduled changes.
    Schedules { id_or_key: String },
    /// Schedule a change.
    Schedule {
        id_or_key: String,
        #[arg(long, value_enum)]
        action: ScheduleAction,
        /// When to apply it (RFC 3339).
        #[arg(long)]
        at: String,
        #[arg(long)]
        reason: String,
        /// Stage id (stage-change).
        #[arg(long)]
        stage: Option<String>,
        /// Requested stage status (stage-change), e.g. DEPLOYED.
        #[arg(long)]
        requested_status: Option<String>,
        /// Extra JSON payload.
        #[arg(long)]
        payload: Option<String>,
        #[arg(long)]
        timezone: Option<String>,
    },
    /// Cancel a scheduled change.
    Unschedule {
        change_id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Move a scheduled change.
    Reschedule {
        change_id: String,
        #[arg(long)]
        at: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        timezone: Option<String>,
    },
    /// List a flag's Jira links.
    Links { id_or_key: String },
    /// Link a flag to a Jira issue.
    Link {
        id_or_key: String,
        /// Issue key such as PROJ-123.
        issue: String,
        /// Link to the issue (`--url` is the backend address).
        #[arg(long)]
        issue_url: Option<String>,
    },
    /// Remove a Jira link.
    Unlink { id_or_key: String, link_id: String },
    /// Find flags with a natural-language query (AI).
    Search {
        query: String,
        #[arg(long)]
        limit: Option<i64>,
    },
}

#[derive(Debug, Clone, Default, Args)]
pub struct FlagFilters {
    /// Name contains.
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub tag: Option<String>,
    #[arg(long)]
    pub owner: Option<String>,
    /// release, experiment, ops, permission, config or unclassified.
    #[arg(long)]
    pub flag_kind: Option<String>,
    /// Only flags linked to this Jira issue.
    #[arg(long)]
    pub external_key: Option<String>,
    #[arg(long)]
    pub lifecycle_stage: Option<String>,
    /// Only stale flags.
    #[arg(long)]
    pub stale: bool,
    /// Include archived flags.
    #[arg(long)]
    pub include_archived: bool,
}

impl FlagFilters {
    /// Query parameters of `GET /teams/{team}/features`.
    pub fn query(&self) -> Vec<(&'static str, String)> {
        let mut query = Vec::new();
        let mut text = |key: &'static str, value: &Option<String>| {
            if let Some(value) = value {
                query.push((key, value.clone()));
            }
        };
        text("name", &self.name);
        text("tag", &self.tag);
        text("owner", &self.owner);
        text("flagKind", &self.flag_kind);
        text("externalKey", &self.external_key);
        text("lifecycleStage", &self.lifecycle_stage);
        if self.stale {
            query.push(("stale", "true".to_string()));
        }
        if self.include_archived {
            query.push(("includeArchived", "true".to_string()));
        }
        query
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum BulkAction {
    UpdateOwner,
    UpdateTags,
    UpdateLifecycle,
    Archive,
    Export,
}

impl BulkAction {
    pub fn api_name(self) -> &'static str {
        match self {
            BulkAction::UpdateOwner => "update_owner",
            BulkAction::UpdateTags => "update_tags",
            BulkAction::UpdateLifecycle => "update_lifecycle",
            BulkAction::Archive => "archive",
            BulkAction::Export => "export",
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ScheduleAction {
    EnableFeature,
    DisableFeature,
    StageChange,
    ArchiveFeature,
}

impl ScheduleAction {
    pub fn api_name(self) -> &'static str {
        match self {
            ScheduleAction::EnableFeature => "ENABLE_FEATURE",
            ScheduleAction::DisableFeature => "DISABLE_FEATURE",
            ScheduleAction::StageChange => "STAGE_CHANGE",
            ScheduleAction::ArchiveFeature => "ARCHIVE_FEATURE",
        }
    }
}

#[derive(Debug, Args)]
pub struct ApprovalsArgs {
    #[command(subcommand)]
    pub command: ApprovalsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ApprovalsSubcommand {
    /// List the team's approval requests.
    List {
        /// Comma-separated statuses: pending, approved, rejected, cancelled, auto_approved.
        #[arg(long, default_value = "pending")]
        status: String,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Approve a request (people only).
    Approve {
        id: String,
        #[arg(long)]
        comment: Option<String>,
    },
    /// Reject a request (people only).
    Reject {
        id: String,
        #[arg(long)]
        comment: Option<String>,
    },
    /// Cancel a request.
    Cancel {
        id: String,
        #[arg(long)]
        comment: Option<String>,
    },
    /// Preview which policy and approvers a change would get (JSON body).
    Preview {
        #[arg(long)]
        data: String,
    },
}

#[derive(Debug, Args)]
pub struct EvaluateArgs {
    /// Flag key.
    #[arg(long = "flag", alias = "feature-key")]
    pub flag: String,
    /// User or entity the flag is evaluated for.
    #[arg(long)]
    pub targeting_key: String,
    /// Evaluation context as a JSON object.
    #[arg(long, default_value = "{}")]
    pub context: String,
    /// Exit 0 when the flag is true and 10 when it is false.
    #[arg(long)]
    pub exit_code: bool,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigSubcommand {
    /// Print the team, its environments and all flags as JSON.
    Export,
}

pub const STAGE_REQUESTS: [&str; 6] = [
    "DEPLOYMENT_REQUESTED",
    "DEPLOYMENT_REJECTED",
    "DEPLOYED",
    "ROLLBACK_REQUESTED",
    "ROLLBACK_REJECTED",
    "ROLLBACKED",
];

#[derive(Debug, Args)]
pub struct RolloutArgs {
    #[command(subcommand)]
    pub command: RolloutSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum RolloutSubcommand {
    /// Request a stage change, by stage id or by flag key and environment.
    Promote {
        /// Stage id; or use --flag with --env.
        stage_id: Option<String>,
        /// Flag key; the stage is found from --env.
        #[arg(long, conflicts_with = "stage_id")]
        flag: Option<String>,
        #[arg(
            long,
            default_value = "DEPLOYMENT_REQUESTED",
            value_parser = clap::builder::PossibleValuesParser::new(STAGE_REQUESTS),
            ignore_case = true
        )]
        request: String,
        /// Why the change is requested.
        #[arg(long)]
        reason: Option<String>,
        /// Ticket or change id, for example a Jira issue key.
        #[arg(long)]
        external_ref: Option<String>,
        /// Reason for changing during a freeze window.
        #[arg(long)]
        freeze_override_reason: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Log in with username and password, even when the session uses SSO.
    #[arg(long, conflicts_with = "sso")]
    pub password: bool,
    /// Log in through SSO provider SLUG in the browser; without SLUG, the
    /// session's sso_provider.
    #[arg(long, value_name = "SLUG", num_args = 0..=1, default_missing_value = "")]
    pub sso: Option<String>,
    /// Approve the login in a browser on any machine with a short code
    /// (for SSH sessions and containers).
    #[arg(long, conflicts_with_all = ["password", "sso"])]
    pub use_device_code: bool,
    /// Do not open a browser; with SSO this switches to the device code.
    #[arg(long)]
    pub no_browser: bool,
    /// Username; asked for when not given.
    #[arg(long)]
    pub username: Option<String>,
}

#[derive(Debug, Args)]
pub struct LogoutArgs {
    /// Log out of every cached session.
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct ConfigureArgs {
    #[command(subcommand)]
    pub command: Option<ConfigureSubcommand>,
}

#[derive(Debug, Subcommand)]
pub enum ConfigureSubcommand {
    /// Set a profile value: session, url, team, environment, output, timeout or token.
    Set { key: String, value: String },
    /// Print a profile value from the files.
    Get { key: String },
    /// Show the resolved settings and where each comes from.
    List,
    /// List profiles.
    ListProfiles,
}

#[derive(Debug, Args)]
pub struct TeamsArgs {
    #[command(subcommand)]
    pub command: TeamsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum TeamsSubcommand {
    /// List your teams; the active one is marked.
    List,
    /// Make a team the profile's default.
    Use {
        /// Team name or id.
        team: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_global_flags_still_parse() {
        let cli = Cli::parse_from([
            "fluxgate",
            "--base-url",
            "http://h/api/v1",
            "--team-id",
            "team-a",
            "health",
        ]);
        assert_eq!(cli.url.as_deref(), Some("http://h/api/v1"));
        assert_eq!(cli.team.as_deref(), Some("team-a"));
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = Cli::parse_from([
            "fluxgate",
            "flags",
            "list",
            "--team-id",
            "team-b",
            "--limit",
            "10",
        ]);
        assert_eq!(cli.team.as_deref(), Some("team-b"));
        match cli.command {
            Command::Flags(FlagsArgs {
                command: FlagsSubcommand::List { page, .. },
            }) => assert_eq!(page.limit, Some(10)),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn json_flag_means_json_output_and_conflicts_with_output() {
        let cli = Cli::parse_from(["fluxgate", "--json", "health"]);
        assert_eq!(cli.overrides().output, Some(OutputFormat::Json));
        assert!(
            Cli::try_parse_from(["fluxgate", "--json", "--output", "table", "health"]).is_err()
        );
    }

    #[test]
    fn all_conflicts_with_limit() {
        assert!(
            Cli::try_parse_from(["fluxgate", "flags", "list", "--all", "--limit", "5"]).is_err()
        );
    }

    #[test]
    fn parses_legacy_evaluate_command_for_ci() {
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
        assert_eq!(cli.environment.as_deref(), Some("env"));
        match cli.command {
            Command::Evaluate(args) => {
                assert_eq!(args.flag, "checkout");
                assert_eq!(args.targeting_key, "user-1");
                assert!(!args.exit_code);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_legacy_rollout_promote_command() {
        let cli = Cli::parse_from([
            "fluxgate",
            "rollout",
            "promote",
            "stage-123",
            "--request",
            "DEPLOYED",
        ]);
        match cli.command {
            Command::Rollout(RolloutArgs {
                command:
                    RolloutSubcommand::Promote {
                        stage_id,
                        request,
                        flag,
                        ..
                    },
            }) => {
                assert_eq!(stage_id.as_deref(), Some("stage-123"));
                assert_eq!(request, "DEPLOYED");
                assert_eq!(flag, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rollout_request_is_checked() {
        assert!(
            Cli::try_parse_from([
                "fluxgate",
                "rollout",
                "promote",
                "s",
                "--request",
                "deployed"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["fluxgate", "rollout", "promote", "s", "--request", "LAUNCH"])
                .is_err()
        );
    }

    #[test]
    fn cli_debug_hides_the_token() {
        let cli = Cli::parse_from(["fluxgate", "--token", "secret-token", "health"]);
        assert!(!format!("{cli:?}").contains("secret-token"));
    }

    #[test]
    fn command_definitions_are_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn subcommand_options_do_not_reuse_global_option_names() {
        use clap::CommandFactory;
        fn walk(command: &clap::Command, globals: &[String], path: &str, found: &mut Vec<String>) {
            for sub in command.get_subcommands() {
                let here = format!("{path} {}", sub.get_name());
                for arg in sub.get_arguments() {
                    if let Some(long) = arg.get_long()
                        && globals.iter().any(|global| global == long)
                    {
                        found.push(format!("{here} --{long}"));
                    }
                }
                walk(sub, globals, &here, found);
            }
        }
        let cli = Cli::command();
        let globals: Vec<String> = cli
            .get_arguments()
            .filter(|arg| arg.is_global_set())
            .filter_map(|arg| arg.get_long().map(str::to_string))
            .collect();
        let mut found = Vec::new();
        walk(&cli, &globals, "fluxgate", &mut found);
        assert!(found.is_empty(), "options shadow global ones: {found:?}");
    }
}
