use crate::Error;
use crate::database::jwt_token::JwtTokenRepository;
use crate::database::refresh_token::RefreshTokenRepository;
use log::{info, warn};
use std::time::Duration;
use tokio::time;

/// Periodically deletes expired session tokens: expired (or long-revoked)
/// access tokens, and refresh tokens that expired more than a day ago.
pub struct TokenCleanupScheduler {
    jwt_token_repository: Box<dyn JwtTokenRepository>,
    refresh_token_repository: Box<dyn RefreshTokenRepository>,
    interval: Duration,
}

impl TokenCleanupScheduler {
    pub fn new(
        jwt_token_repository: Box<dyn JwtTokenRepository>,
        refresh_token_repository: Box<dyn RefreshTokenRepository>,
        interval: Duration,
    ) -> Self {
        Self {
            jwt_token_repository,
            refresh_token_repository,
            interval,
        }
    }

    pub async fn start(self) {
        let mut ticker = time::interval(self.interval);
        loop {
            ticker.tick().await;
            match self.run_once().await {
                Ok((access, refresh)) if access + refresh > 0 => {
                    info!(
                        "Token cleanup deleted {} access token(s) and {} refresh token(s)",
                        access, refresh
                    );
                }
                Ok(_) => {}
                Err(err) => warn!("Token cleanup scheduler encountered an error: {err}"),
            }
        }
    }

    /// Returns the number of deleted (access, refresh) tokens.
    pub async fn run_once(&self) -> Result<(u64, u64), Error> {
        let access = self.jwt_token_repository.cleanup_expired_tokens().await?;
        let refresh = self.refresh_token_repository.delete_expired().await?;
        Ok((access, refresh))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::jwt_token::MockJwtTokenRepository;
    use crate::database::refresh_token::MockRefreshTokenRepository;

    #[tokio::test]
    async fn run_once_cleans_access_and_refresh_tokens() {
        let mut jwt = MockJwtTokenRepository::new();
        jwt.expect_cleanup_expired_tokens()
            .times(1)
            .returning(|| Ok(2));
        let mut refresh = MockRefreshTokenRepository::new();
        refresh.expect_delete_expired().times(1).returning(|| Ok(5));

        let scheduler =
            TokenCleanupScheduler::new(Box::new(jwt), Box::new(refresh), Duration::from_secs(3600));
        assert_eq!(scheduler.run_once().await.unwrap(), (2, 5));
    }
}
