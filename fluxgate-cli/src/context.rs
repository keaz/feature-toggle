//! Authenticated API access plus team and environment resolution.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::ApiClient;
use crate::auth::claims::{TokenClaims, decode_claims};
use crate::auth::session::SessionStore;
use crate::config::{Credential, Paths, Settings};
use crate::error::CliError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub active: bool,
}

pub fn is_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value.trim()).is_ok()
}

pub fn team_names(teams: &[Team]) -> String {
    teams.iter().map(|team| team.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// The one team whose name matches `wanted`, ignoring case.
pub fn find_team(teams: &[Team], wanted: &str) -> Result<Team, CliError> {
    let wanted = wanted.trim();
    let matches: Vec<&Team> = teams.iter().filter(|team| team.name.eq_ignore_ascii_case(wanted)).collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(CliError::Usage(format!("team '{wanted}' not found; available: {}", team_names(teams)))),
        many => Err(CliError::Usage(format!(
            "team name '{wanted}' matches several teams ({}); use the team id",
            many.iter().map(|team| team.id.as_str()).collect::<Vec<_>>().join(", ")
        ))),
    }
}

#[derive(Debug)]
pub struct Context {
    pub settings: Settings,
    pub paths: Paths,
    pub api: ApiClient,
    pub claims: Option<TokenClaims>,
}

impl Context {
    pub fn timeout(settings: &Settings) -> Duration {
        Duration::from_secs(settings.timeout.value)
    }

    /// A client without credentials, for public routes.
    pub fn anonymous(settings: Settings, paths: Paths) -> Result<Self, CliError> {
        let api = ApiClient::new(&settings.url.value, None, Self::timeout(&settings))?;
        Ok(Self { settings, paths, api, claims: None })
    }

    pub async fn connect(settings: Settings, paths: Paths) -> Result<Self, CliError> {
        let token = match &settings.credential {
            Credential::Static { token, .. } => token.clone(),
            Credential::Session { name } => {
                SessionStore::new(paths.sessions.clone())
                    .access_token(name, &settings.profile, &settings.url.value, Self::timeout(&settings))
                    .await?
            }
            Credential::None => {
                return Err(CliError::Auth(format!(
                    "no credentials for profile {}: run fluxgate configure or fluxgate login",
                    settings.profile
                )));
            }
        };
        let claims = decode_claims(&token);
        let api = ApiClient::new(&settings.url.value, Some(token), Self::timeout(&settings))?;
        Ok(Self { settings, paths, api, claims })
    }

    /// The team a system-client token is bound to.
    pub fn token_team(&self) -> Option<&str> {
        self.claims
            .as_ref()
            .filter(|claims| claims.is_system_client())
            .and_then(|claims| claims.team_id.as_deref())
    }

    pub async fn teams(&self) -> Result<Vec<Team>, CliError> {
        let value = self.api.get(&["teams"], &[]).await?;
        serde_json::from_value(value)
            .map_err(|err| CliError::Other(format!("unexpected teams response: {err}")))
    }

    /// Team id from the settings (name or id) or from a system-client token.
    pub async fn team_id(&self) -> Result<String, CliError> {
        let token_team = self.token_team().map(str::to_string);
        let Some(wanted) = self.settings.team.as_ref().map(|team| team.value.clone()) else {
            return token_team.ok_or_else(|| {
                CliError::Usage(
                    "team required: pass --team, set FLUXGATE_TEAM, or run fluxgate teams use <name>".into(),
                )
            });
        };
        let id = if is_uuid(&wanted) {
            wanted.trim().to_string()
        } else {
            find_team(&self.teams().await?, &wanted)?.id
        };
        // The server answers for the token's team whatever the request says,
        // so a different team would silently act on the wrong one.
        if let Some(token_team) = token_team
            && !token_team.eq_ignore_ascii_case(&id)
        {
            return Err(CliError::Usage(format!(
                "team '{wanted}' does not match the system-client token's team {token_team}"
            )));
        }
        Ok(id)
    }

    pub async fn environments(&self, team_id: &str) -> Result<Vec<Environment>, CliError> {
        let items = self.api.get_all_pages(&["teams", team_id, "environments"], &[]).await?;
        serde_json::from_value(Value::Array(items))
            .map_err(|err| CliError::Other(format!("unexpected environments response: {err}")))
    }

    pub async fn environment_id(&self, team_id: &str) -> Result<String, CliError> {
        let wanted = self.wanted_environment()?;
        if is_uuid(&wanted) {
            return Ok(wanted);
        }
        let environments = self.environments(team_id).await?;
        environments
            .iter()
            .find(|environment| environment.name.eq_ignore_ascii_case(&wanted))
            .map(|environment| environment.id.clone())
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "environment '{wanted}' not found; available: {}",
                    environments.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(", ")
                ))
            })
    }

    /// Environment name, for routes that address environments by name.
    pub async fn environment_name(&self, team_id: &str) -> Result<String, CliError> {
        let wanted = self.wanted_environment()?;
        if !is_uuid(&wanted) {
            return Ok(wanted);
        }
        self.environments(team_id)
            .await?
            .into_iter()
            .find(|environment| environment.id.eq_ignore_ascii_case(&wanted))
            .map(|environment| environment.name)
            .ok_or_else(|| CliError::Usage(format!("environment {wanted} not found in team {team_id}")))
    }

    fn wanted_environment(&self) -> Result<String, CliError> {
        self.settings
            .environment
            .as_ref()
            .map(|environment| environment.value.trim().to_string())
            .ok_or_else(|| CliError::Usage("environment required: pass --env or set FLUXGATE_ENVIRONMENT".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigFiles, Env, Overrides, resolve};
    use crate::error::{EXIT_AUTH, EXIT_USAGE};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEAM_A: &str = "11111111-1111-1111-1111-111111111111";
    const TEAM_B: &str = "22222222-2222-2222-2222-222222222222";
    const ENV_ID: &str = "33333333-3333-3333-3333-333333333333";

    fn jwt(claims: serde_json::Value) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    async fn context(server: &MockServer, token: &str, pairs: &[(&str, &str)]) -> Context {
        let url = format!("{}/api/v1", server.uri());
        let mut all = vec![("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token)];
        all.extend_from_slice(pairs);
        let settings = resolve(&ConfigFiles::empty(), &Env::from_pairs(all), &Overrides::default(), false).unwrap();
        let dir = std::env::temp_dir();
        let paths = Paths { config: dir.join("c"), credentials: dir.join("k"), sessions: dir.join("s") };
        Context::connect(settings, paths).await.unwrap()
    }

    fn user() -> String {
        jwt(json!({ "sub": "u1", "username": "alice", "exp": 4102444800i64 }))
    }

    fn system(team: &str) -> String {
        jwt(json!({ "sub": "sc1", "username": "ci", "exp": 4102444800i64, "token_type": "system_client", "team_id": team }))
    }

    async fn mount_teams(server: &MockServer, teams: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/api/v1/teams"))
            .respond_with(ResponseTemplate::new(200).set_body_json(teams))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn uuid_team_is_used_without_a_request() {
        let server = MockServer::start().await;
        let ctx = context(&server, &user(), &[("FLUXGATE_TEAM", TEAM_A)]).await;
        assert_eq!(ctx.team_id().await.unwrap(), TEAM_A);
    }

    #[tokio::test]
    async fn team_name_is_matched_ignoring_case() {
        let server = MockServer::start().await;
        mount_teams(&server, json!([{ "id": TEAM_A, "name": "Payments" }, { "id": TEAM_B, "name": "Checkout" }])).await;
        let ctx = context(&server, &user(), &[("FLUXGATE_TEAM", "payments")]).await;
        assert_eq!(ctx.team_id().await.unwrap(), TEAM_A);
    }

    #[tokio::test]
    async fn unknown_team_lists_the_available_names() {
        let server = MockServer::start().await;
        mount_teams(&server, json!([{ "id": TEAM_A, "name": "Payments" }])).await;
        let ctx = context(&server, &user(), &[("FLUXGATE_TEAM", "billing")]).await;
        let err = ctx.team_id().await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.to_string().contains("available: Payments"));
    }

    #[tokio::test]
    async fn duplicate_team_names_ask_for_the_id() {
        let server = MockServer::start().await;
        mount_teams(&server, json!([{ "id": TEAM_A, "name": "Ops" }, { "id": TEAM_B, "name": "ops" }])).await;
        let ctx = context(&server, &user(), &[("FLUXGATE_TEAM", "OPS")]).await;
        let err = ctx.team_id().await.unwrap_err();
        assert!(err.to_string().contains(TEAM_A) && err.to_string().contains(TEAM_B));
    }

    #[tokio::test]
    async fn missing_team_for_a_user_token_is_a_usage_error() {
        let server = MockServer::start().await;
        let ctx = context(&server, &user(), &[]).await;
        assert!(ctx.team_id().await.unwrap_err().to_string().starts_with("team required"));
    }

    #[tokio::test]
    async fn system_token_supplies_and_guards_the_team() {
        let server = MockServer::start().await;
        let ctx = context(&server, &system(TEAM_A), &[]).await;
        assert_eq!(ctx.token_team(), Some(TEAM_A));
        assert_eq!(ctx.team_id().await.unwrap(), TEAM_A);
        let ctx = context(&server, &system(TEAM_A), &[("FLUXGATE_TEAM", TEAM_B)]).await;
        let err = ctx.team_id().await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.to_string().contains("does not match the system-client token's team"));
    }

    #[tokio::test]
    async fn environment_names_and_ids_are_resolved() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [{ "id": ENV_ID, "name": "staging", "teamId": TEAM_A, "active": true, "environmentType": "STAGING" }],
                "meta": { "offset": 0, "limit": 200, "total": 1 }
            })))
            .mount(&server)
            .await;
        let ctx = context(&server, &user(), &[("FLUXGATE_ENVIRONMENT", "Staging")]).await;
        assert_eq!(ctx.environment_id(TEAM_A).await.unwrap(), ENV_ID);
        assert_eq!(ctx.environment_name(TEAM_A).await.unwrap(), "Staging");
        let ctx = context(&server, &user(), &[("FLUXGATE_ENVIRONMENT", ENV_ID)]).await;
        assert_eq!(ctx.environment_id(TEAM_A).await.unwrap(), ENV_ID);
        assert_eq!(ctx.environment_name(TEAM_A).await.unwrap(), "staging");
        let ctx = context(&server, &user(), &[("FLUXGATE_ENVIRONMENT", "prod")]).await;
        assert!(ctx.environment_id(TEAM_A).await.unwrap_err().to_string().contains("available: staging"));
    }

    #[tokio::test]
    async fn connect_without_credentials_is_an_auth_error() {
        let settings = resolve(&ConfigFiles::empty(), &Env::default(), &Overrides::default(), false).unwrap();
        let dir = std::env::temp_dir();
        let paths = Paths { config: dir.join("c"), credentials: dir.join("k"), sessions: dir.join("s") };
        let err = Context::connect(settings, paths).await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_AUTH);
        assert_eq!(err.to_string(), "no credentials for profile default: run fluxgate configure or fluxgate login");
    }
}
