use crate::Error;
use crate::database::jira_event::JiraEventRepository;
use crate::database::jwt_token::JwtTokenRepository;
use crate::database::refresh_token::RefreshTokenRepository;
use crate::database::sso_login_code::SsoLoginCodeRepository;
use crate::database::sso_login_state::SsoLoginStateRepository;
use log::{info, warn};
use std::time::Duration;
use tokio::time;

/// Periodically deletes expired session tokens: expired (or long-revoked)
/// access tokens, refresh tokens that expired more than a day ago, expired SSO
/// login states and SSO exchange codes that expired more than a day ago. With
/// [`TokenCleanupScheduler::with_jira_events`], also Jira events older than
/// [`JIRA_EVENT_RETENTION_DAYS`].
pub struct TokenCleanupScheduler {
    jwt_token_repository: Box<dyn JwtTokenRepository>,
    refresh_token_repository: Box<dyn RefreshTokenRepository>,
    sso_login_state_repository: Box<dyn SsoLoginStateRepository>,
    sso_login_code_repository: Box<dyn SsoLoginCodeRepository>,
    jira_event_repository: Option<Box<dyn JiraEventRepository>>,
    interval: Duration,
}

/// Days a Jira event stays in the event log (JI-15).
pub const JIRA_EVENT_RETENTION_DAYS: i64 = 30;

/// Rows deleted by one cleanup run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CleanupCounts {
    pub access_tokens: u64,
    pub refresh_tokens: u64,
    pub sso_login_states: u64,
    pub sso_login_codes: u64,
    pub jira_events: u64,
}

impl CleanupCounts {
    fn total(&self) -> u64 {
        self.access_tokens
            + self.refresh_tokens
            + self.sso_login_states
            + self.sso_login_codes
            + self.jira_events
    }
}

impl TokenCleanupScheduler {
    pub fn new(
        jwt_token_repository: Box<dyn JwtTokenRepository>,
        refresh_token_repository: Box<dyn RefreshTokenRepository>,
        sso_login_state_repository: Box<dyn SsoLoginStateRepository>,
        sso_login_code_repository: Box<dyn SsoLoginCodeRepository>,
        interval: Duration,
    ) -> Self {
        Self {
            jwt_token_repository,
            refresh_token_repository,
            sso_login_state_repository,
            sso_login_code_repository,
            jira_event_repository: None,
            interval,
        }
    }

    /// Also deletes Jira events older than [`JIRA_EVENT_RETENTION_DAYS`].
    pub fn with_jira_events(mut self, repository: Box<dyn JiraEventRepository>) -> Self {
        self.jira_event_repository = Some(repository);
        self
    }

    pub async fn start(self) {
        let mut ticker = time::interval(self.interval);
        loop {
            ticker.tick().await;
            match self.run_once().await {
                Ok(counts) if counts.total() > 0 => {
                    info!(
                        "Token cleanup deleted {} access token(s), {} refresh token(s), {} SSO login state(s), {} SSO login code(s) and {} Jira event(s)",
                        counts.access_tokens,
                        counts.refresh_tokens,
                        counts.sso_login_states,
                        counts.sso_login_codes,
                        counts.jira_events
                    );
                }
                Ok(_) => {}
                Err(err) => warn!("Token cleanup scheduler encountered an error: {err}"),
            }
        }
    }

    pub async fn run_once(&self) -> Result<CleanupCounts, Error> {
        Ok(CleanupCounts {
            access_tokens: self.jwt_token_repository.cleanup_expired_tokens().await?,
            refresh_tokens: self.refresh_token_repository.delete_expired().await?,
            sso_login_states: self
                .sso_login_state_repository
                .delete_expired_states()
                .await?,
            sso_login_codes: self
                .sso_login_code_repository
                .delete_expired_codes()
                .await?,
            jira_events: match &self.jira_event_repository {
                Some(repository) => {
                    repository
                        .delete_older_than(
                            chrono::Utc::now() - chrono::Duration::days(JIRA_EVENT_RETENTION_DAYS),
                        )
                        .await?
                }
                None => 0,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::jira_event::MockJiraEventRepository;
    use crate::database::jwt_token::MockJwtTokenRepository;
    use crate::database::refresh_token::MockRefreshTokenRepository;
    use crate::database::sso_login_code::MockSsoLoginCodeRepository;
    use crate::database::sso_login_state::MockSsoLoginStateRepository;

    #[tokio::test]
    async fn run_once_cleans_tokens_and_sso_login_rows() {
        let mut jwt = MockJwtTokenRepository::new();
        jwt.expect_cleanup_expired_tokens()
            .times(1)
            .returning(|| Ok(2));
        let mut refresh = MockRefreshTokenRepository::new();
        refresh.expect_delete_expired().times(1).returning(|| Ok(5));
        let mut states = MockSsoLoginStateRepository::new();
        states
            .expect_delete_expired_states()
            .times(1)
            .returning(|| Ok(3));
        let mut codes = MockSsoLoginCodeRepository::new();
        codes
            .expect_delete_expired_codes()
            .times(1)
            .returning(|| Ok(4));

        let mut jira_events = MockJiraEventRepository::new();
        jira_events
            .expect_delete_older_than()
            .withf(|cutoff| {
                let age = chrono::Utc::now() - *cutoff;
                age >= chrono::Duration::days(JIRA_EVENT_RETENTION_DAYS)
                    && age
                        < chrono::Duration::days(JIRA_EVENT_RETENTION_DAYS)
                            + chrono::Duration::minutes(1)
            })
            .times(1)
            .returning(|_| Ok(6));

        let scheduler = TokenCleanupScheduler::new(
            Box::new(jwt),
            Box::new(refresh),
            Box::new(states),
            Box::new(codes),
            Duration::from_secs(3600),
        )
        .with_jira_events(Box::new(jira_events));
        assert_eq!(
            scheduler.run_once().await.unwrap(),
            CleanupCounts {
                access_tokens: 2,
                refresh_tokens: 5,
                sso_login_states: 3,
                sso_login_codes: 4,
                jira_events: 6,
            }
        );
    }
}
