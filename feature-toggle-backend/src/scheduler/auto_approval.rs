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
            if let Err(err) = self.reconcile_capped_requests().await {
                warn!("Approval reconciliation encountered an error: {err}");
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
    /// Approves pending requests whose AI-11 override can no longer be reached
    /// (no remaining eligible approver can vote) while their approvals already
    /// meet the policy. Returns how many were approved.
    pub async fn reconcile_capped_requests(&self) -> Result<usize, Error> {
        let requests = self
            .approval_repository
            .list_capped_requests_ready_for_approval()
            .await?;

        let mut approved = 0;
        for request in requests {
            match self.approval_logic.approve_capped_request(request).await {
                Ok(Some(_)) => approved += 1,
                Ok(None) => {}
                Err(err) => warn!("Failed to reconcile approval request: {err}"),
            }
        }

        Ok(approved)
    }
}
