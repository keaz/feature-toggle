//! System-client tokens, JWT signing secrets, SSO, users and notifications.

use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::json;

use super::{App, call, json_data, optional_data};
use crate::error::CliError;
use crate::output::Outcome;

#[derive(Debug, Args)]
pub struct SystemClientsArgs {
    #[command(subcommand)]
    pub command: SystemClientsCommand,
}

#[derive(Debug, Subcommand)]
pub enum SystemClientsCommand {
    /// List a system client's tokens.
    Tokens { client_id: String },
    /// Create a token (shown once).
    CreateToken {
        client_id: String,
        #[arg(long)]
        data: Option<String>,
    },
    /// Revoke a token.
    RevokeToken { token_id: String },
    /// Replace the client's token (shown once).
    RotateToken { client_id: String },
}

#[derive(Debug, Args)]
pub struct JwtSecretsArgs {
    #[command(subcommand)]
    pub command: JwtSecretsCommand,
}

#[derive(Debug, Subcommand)]
pub enum JwtSecretsCommand {
    /// List signing secrets.
    List,
    /// Create a new signing secret.
    Rotate,
    /// Deactivate every secret (ends all sessions).
    DeactivateAll,
}

#[derive(Debug, Args)]
pub struct SsoArgs {
    #[command(subcommand)]
    pub command: SsoCommand,
}

#[derive(Debug, Subcommand)]
pub enum SsoCommand {
    /// Show a provider's group mappings.
    Mappings { provider_id: String },
    /// Replace a provider's group mappings (JSON body).
    SetMappings {
        provider_id: String,
        #[arg(long)]
        data: String,
    },
    /// Test a provider's configuration.
    Test { provider_id: String },
    /// Show SSO settings.
    Settings,
    /// Replace SSO settings (JSON body).
    SetSettings {
        #[arg(long)]
        data: String,
    },
}

#[derive(Debug, Args)]
pub struct UsersArgs {
    #[command(subcommand)]
    pub command: UsersCommand,
}

#[derive(Debug, Subcommand)]
pub enum UsersCommand {
    /// Show a user's roles.
    Roles { user_id: String },
    /// Assign roles (JSON body).
    SetRoles {
        user_id: String,
        #[arg(long)]
        data: String,
    },
    /// Assign teams (JSON body).
    SetTeams {
        user_id: String,
        #[arg(long)]
        data: String,
    },
    /// Set a temporary password (JSON body).
    TemporaryPassword {
        user_id: String,
        #[arg(long)]
        data: String,
    },
}

#[derive(Debug, Args)]
pub struct NotificationsArgs {
    #[command(subcommand)]
    pub command: NotificationsCommand,
}

#[derive(Debug, Subcommand)]
pub enum NotificationsCommand {
    /// Show notification settings.
    Show,
    /// Configure a channel (JSON body).
    Channel {
        channel: String,
        #[arg(long)]
        data: String,
    },
    /// Configure a notification type (JSON body).
    Preference {
        notification_type: String,
        #[arg(long)]
        data: String,
    },
}

pub async fn system_clients(
    args: SystemClientsArgs,
    app: &mut App<'_>,
) -> Result<Outcome, CliError> {
    let data = match &args.command {
        SystemClientsCommand::CreateToken { data, .. } => Some(optional_data(data.as_deref())?),
        _ => Some(json!({})),
    };
    let context = app.connect().await?;
    match &args.command {
        SystemClientsCommand::Tokens { client_id } => {
            call(
                &context,
                Method::GET,
                &["system-clients", client_id, "tokens"],
                &[],
                None,
            )
            .await
        }
        SystemClientsCommand::CreateToken { client_id, .. } => {
            call(
                &context,
                Method::POST,
                &["system-clients", client_id, "tokens"],
                &[],
                data,
            )
            .await
        }
        SystemClientsCommand::RevokeToken { token_id } => {
            call(
                &context,
                Method::POST,
                &["system-client-tokens", token_id, "revoke"],
                &[],
                data,
            )
            .await
        }
        SystemClientsCommand::RotateToken { client_id } => {
            call(
                &context,
                Method::POST,
                &["system-clients", client_id, "regenerate-token"],
                &[],
                data,
            )
            .await
        }
    }
}

pub async fn jwt_secrets(args: JwtSecretsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    match args.command {
        JwtSecretsCommand::List => {
            call(&context, Method::GET, &["auth", "jwt-secrets"], &[], None).await
        }
        JwtSecretsCommand::Rotate => {
            call(
                &context,
                Method::POST,
                &["auth", "jwt-secrets"],
                &[],
                Some(json!({})),
            )
            .await
        }
        JwtSecretsCommand::DeactivateAll => {
            call(
                &context,
                Method::POST,
                &["auth", "jwt-secrets", "deactivate-all"],
                &[],
                Some(json!({})),
            )
            .await
        }
    }
}

pub async fn sso(args: SsoArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let data = match &args.command {
        SsoCommand::SetMappings { data, .. } | SsoCommand::SetSettings { data } => {
            Some(json_data(data)?)
        }
        _ => None,
    };
    let context = app.connect().await?;
    match &args.command {
        SsoCommand::Mappings { provider_id } => {
            call(
                &context,
                Method::GET,
                &["sso", "providers", provider_id, "mappings"],
                &[],
                None,
            )
            .await
        }
        SsoCommand::SetMappings { provider_id, .. } => {
            call(
                &context,
                Method::PUT,
                &["sso", "providers", provider_id, "mappings"],
                &[],
                data,
            )
            .await
        }
        SsoCommand::Test { provider_id } => {
            call(
                &context,
                Method::POST,
                &["sso", "providers", provider_id, "test"],
                &[],
                Some(json!({})),
            )
            .await
        }
        SsoCommand::Settings => call(&context, Method::GET, &["sso", "settings"], &[], None).await,
        SsoCommand::SetSettings { .. } => {
            call(&context, Method::PUT, &["sso", "settings"], &[], data).await
        }
    }
}

pub async fn users(args: UsersArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let data = match &args.command {
        UsersCommand::SetRoles { data, .. }
        | UsersCommand::SetTeams { data, .. }
        | UsersCommand::TemporaryPassword { data, .. } => Some(json_data(data)?),
        UsersCommand::Roles { .. } => None,
    };
    let context = app.connect().await?;
    match &args.command {
        UsersCommand::Roles { user_id } => {
            call(
                &context,
                Method::GET,
                &["users", user_id, "roles"],
                &[],
                None,
            )
            .await
        }
        UsersCommand::SetRoles { user_id, .. } => {
            call(
                &context,
                Method::POST,
                &["users", user_id, "roles"],
                &[],
                data,
            )
            .await
        }
        UsersCommand::SetTeams { user_id, .. } => {
            call(
                &context,
                Method::POST,
                &["users", user_id, "teams"],
                &[],
                data,
            )
            .await
        }
        UsersCommand::TemporaryPassword { user_id, .. } => {
            call(
                &context,
                Method::POST,
                &["auth", "users", user_id, "temporary-password"],
                &[],
                data,
            )
            .await
        }
    }
}

pub async fn notifications(
    args: NotificationsArgs,
    app: &mut App<'_>,
) -> Result<Outcome, CliError> {
    let data = match &args.command {
        NotificationsCommand::Channel { data, .. }
        | NotificationsCommand::Preference { data, .. } => Some(json_data(data)?),
        NotificationsCommand::Show => None,
    };
    let context = app.connect().await?;
    match &args.command {
        NotificationsCommand::Show => {
            call(
                &context,
                Method::GET,
                &["notifications", "settings"],
                &[],
                None,
            )
            .await
        }
        NotificationsCommand::Channel { channel, .. } => {
            call(
                &context,
                Method::PUT,
                &["notifications", "channels", channel],
                &[],
                data,
            )
            .await
        }
        NotificationsCommand::Preference {
            notification_type, ..
        } => {
            call(
                &context,
                Method::PUT,
                &["notifications", "preferences", notification_type],
                &[],
                data,
            )
            .await
        }
    }
}
