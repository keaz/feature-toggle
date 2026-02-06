use std::str::FromStr;

/// Stage status constants to avoid magic strings throughout the codebase
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageStatus {
    DeploymentRequested,
    DeploymentApproved,
    DeploymentRejected,
    Deployed,
    RollbackRequested,
    RollbackApproved,
    RollbackRejected,
    Rollbacked,
}

impl StageStatus {
    /// Convert enum variant to string representation used in the database
    pub fn as_str(&self) -> &'static str {
        match self {
            StageStatus::DeploymentRequested => "DEPLOYMENT_REQUESTED",
            StageStatus::DeploymentApproved => "DEPLOYMENT_APPROVED",
            StageStatus::DeploymentRejected => "DEPLOYMENT_REJECTED",
            StageStatus::Deployed => "DEPLOYED",
            StageStatus::RollbackRequested => "ROLLBACK_REQUESTED",
            StageStatus::RollbackApproved => "ROLLBACK_APPROVED",
            StageStatus::RollbackRejected => "ROLLBACK_REJECTED",
            StageStatus::Rollbacked => "ROLLBACKED",
        }
    }

    /// Parse string representation back to enum variant
    pub fn parse(status: &str) -> Option<Self> {
        status.parse().ok()
    }
}

impl std::fmt::Display for StageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for StageStatus {
    type Err = ();

    fn from_str(status: &str) -> Result<Self, Self::Err> {
        match status {
            "DEPLOYMENT_REQUESTED" => Ok(StageStatus::DeploymentRequested),
            "DEPLOYMENT_APPROVED" => Ok(StageStatus::DeploymentApproved),
            "DEPLOYMENT_REJECTED" => Ok(StageStatus::DeploymentRejected),
            "DEPLOYED" => Ok(StageStatus::Deployed),
            "ROLLBACK_REQUESTED" => Ok(StageStatus::RollbackRequested),
            "ROLLBACK_APPROVED" => Ok(StageStatus::RollbackApproved),
            "ROLLBACK_REJECTED" => Ok(StageStatus::RollbackRejected),
            "ROLLBACKED" => Ok(StageStatus::Rollbacked),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_status_as_str() {
        assert_eq!(
            StageStatus::DeploymentRequested.as_str(),
            "DEPLOYMENT_REQUESTED"
        );
        assert_eq!(
            StageStatus::DeploymentApproved.as_str(),
            "DEPLOYMENT_APPROVED"
        );
        assert_eq!(StageStatus::Deployed.as_str(), "DEPLOYED");
        assert_eq!(StageStatus::RollbackRejected.as_str(), "ROLLBACK_REJECTED");
    }

    #[test]
    fn test_stage_status_from_str() {
        assert_eq!(
            StageStatus::parse("DEPLOYMENT_REQUESTED"),
            Some(StageStatus::DeploymentRequested)
        );
        assert_eq!(
            StageStatus::parse("DEPLOYMENT_APPROVED"),
            Some(StageStatus::DeploymentApproved)
        );
        assert_eq!(StageStatus::parse("DEPLOYED"), Some(StageStatus::Deployed));
        assert_eq!(StageStatus::parse("INVALID"), None);
    }

    #[test]
    fn test_stage_status_display() {
        assert_eq!(
            format!("{}", StageStatus::DeploymentRequested),
            "DEPLOYMENT_REQUESTED"
        );
        assert_eq!(format!("{}", StageStatus::Rollbacked), "ROLLBACKED");
    }

    #[test]
    fn test_round_trip_conversion() {
        let statuses = [
            StageStatus::DeploymentRequested,
            StageStatus::DeploymentApproved,
            StageStatus::DeploymentRejected,
            StageStatus::Deployed,
            StageStatus::RollbackRequested,
            StageStatus::RollbackApproved,
            StageStatus::RollbackRejected,
            StageStatus::Rollbacked,
        ];

        for status in statuses {
            let str_repr = status.as_str();
            let parsed = StageStatus::parse(str_repr).unwrap();
            assert_eq!(status, parsed);
        }
    }
}
