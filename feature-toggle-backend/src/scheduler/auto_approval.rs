use crate::Error;
use crate::database::approval::ApprovalRepository;
use crate::logic::approval::ApprovalLogic;
use log::warn;
use std::time::Duration;
use tokio::time;

pub struct AutoApprovalScheduler {
    approval_repository: Box<dyn ApprovalRepository>,
    approval_logic: Box<dyn ApprovalLogic>,
    interval: Duration,
}

impl AutoApprovalScheduler {
    pub fn new(
        approval_repository: Box<dyn ApprovalRepository>,
        approval_logic: Box<dyn ApprovalLogic>,
        interval: Duration,
    ) -> Self {
        Self {
            approval_repository,
            approval_logic,
            interval,
        }
    }

    pub async fn start(self) {
        let mut ticker = time::interval(self.interval);
        loop {
            ticker.tick().await;
            if let Err(err) = self.run_pending().await {
                warn!("Auto approval scheduler encountered an error: {err}");
            }
        }
    }

    pub async fn run_pending(&self) -> Result<(), Error> {
        let requests = self
            .approval_repository
            .list_requests_due_for_auto_approval()
            .await?;

        for request in requests {
            if let Err(err) = self.approval_logic.auto_approve_request(request).await {
                warn!("Failed to auto-approve request: {err}");
            }
        }

        Ok(())
    }
}
