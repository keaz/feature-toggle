//! Command line definitions.

use clap::{Args, Parser, Subcommand};

use crate::config::Overrides;
use crate::output::OutputFormat;

#[derive(Debug, Parser)]
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
    },
    /// Show one flag by id or key.
    Get {
        /// Feature id (UUID) or key.
        id_or_key: String,
    },
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
    /// Log in with username and password.
    #[arg(long)]
    pub password: bool,
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
                command: FlagsSubcommand::List { page },
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
}
