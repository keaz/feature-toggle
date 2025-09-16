/// Stage status constants to avoid magic strings throughout the codebase
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageStatus {
    DeploymentRequested,
    DeploymentRejected,
    Deployed,
    RollbackRequested,
    RollbackRejected,
    Rollbacked,
}

impl StageStatus {
    /// Convert enum variant to string representation used in the database
    pub fn as_str(&self) -> &'static str {
        match self {
            StageStatus::DeploymentRequested => "DEPLOYMENT_REQUESTED",
            StageStatus::DeploymentRejected => "DEPLOYMENT_REJECTED",
            StageStatus::Deployed => "DEPLOYED",
            StageStatus::RollbackRequested => "ROLLBACK_REQUESTED",
            StageStatus::RollbackRejected => "ROLLBACK_REJECTED",
            StageStatus::Rollbacked => "ROLLBACKED",
        }
    }

    /// Parse string representation back to enum variant
    pub fn from_str(status: &str) -> Option<Self> {
        match status {
            "DEPLOYMENT_REQUESTED" => Some(StageStatus::DeploymentRequested),
            "DEPLOYMENT_REJECTED" => Some(StageStatus::DeploymentRejected),
            "DEPLOYED" => Some(StageStatus::Deployed),
            "ROLLBACK_REQUESTED" => Some(StageStatus::RollbackRequested),
            "ROLLBACK_REJECTED" => Some(StageStatus::RollbackRejected),
            "ROLLBACKED" => Some(StageStatus::Rollbacked),
            _ => None,
        }
    }
}

impl std::fmt::Display for StageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_status_as_str() {
        assert_eq!(StageStatus::DeploymentRequested.as_str(), "DEPLOYMENT_REQUESTED");
        assert_eq!(StageStatus::Deployed.as_str(), "DEPLOYED");
        assert_eq!(StageStatus::RollbackRejected.as_str(), "ROLLBACK_REJECTED");
    }

    #[test]
    fn test_stage_status_from_str() {
        assert_eq!(StageStatus::from_str("DEPLOYMENT_REQUESTED"), Some(StageStatus::DeploymentRequested));
        assert_eq!(StageStatus::from_str("DEPLOYED"), Some(StageStatus::Deployed));
        assert_eq!(StageStatus::from_str("INVALID"), None);
    }

    #[test]
    fn test_stage_status_display() {
        assert_eq!(format!("{}", StageStatus::DeploymentRequested), "DEPLOYMENT_REQUESTED");
        assert_eq!(format!("{}", StageStatus::Rollbacked), "ROLLBACKED");
    }

    #[test]
    fn test_round_trip_conversion() {
        let statuses = [
            StageStatus::DeploymentRequested,
            StageStatus::DeploymentRejected,
            StageStatus::Deployed,
            StageStatus::RollbackRequested,
            StageStatus::RollbackRejected,
            StageStatus::Rollbacked,
        ];

        for status in statuses {
            let str_repr = status.as_str();
            let parsed = StageStatus::from_str(str_repr).unwrap();
            assert_eq!(status, parsed);
        }
    }
}