use crate::Error;

/// Authorization helper functions for role-based access control
pub struct RoleAuthorizer;

impl RoleAuthorizer {
    /// Check if user has the Requester role
    pub fn has_requester_role(user_roles: &[String]) -> bool {
        user_roles.iter().any(|role| role == "Requester")
    }

    /// Check if user has the Approver role
    pub fn has_approver_role(user_roles: &[String]) -> bool {
        user_roles.iter().any(|role| role == "Approver")
    }

    /// Authorize stage change request based on user roles and request type
    pub fn authorize_stage_change_request(
        user_roles: &[String],
        request_type: &str,
    ) -> Result<(), Error> {
        match request_type {
            // Requester role operations
            "DEPLOYMENT_REQUESTED" | "ROLLBACK_REQUESTED" => {
                if Self::has_requester_role(user_roles) {
                    Ok(())
                } else {
                    Err(Error::InvalidInput(
                        "Only users with 'Requester' role can request deployments or rollbacks"
                            .to_string(),
                    ))
                }
            }
            // Approver role operations
            "DEPLOYMENT_REJECTED" | "ROLLBACK_REJECTED" => {
                if Self::has_approver_role(user_roles) {
                    Ok(())
                } else {
                    Err(Error::InvalidInput(
                        "Only users with 'Approver' role can approve or reject requests"
                            .to_string(),
                    ))
                }
            }
            // Finalize deployment/rollback after approvals: either Requester or Approver can execute
            "DEPLOYED" | "ROLLBACKED" => {
                if Self::has_requester_role(user_roles) || Self::has_approver_role(user_roles) {
                    Ok(())
                } else {
                    Err(Error::InvalidInput(
                        "Only users with 'Requester' or 'Approver' role can execute deployments or rollbacks"
                            .to_string(),
                    ))
                }
            }
            _ => Err(Error::InvalidInput(format!(
                "Unknown stage change request type: {}",
                request_type
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_requester_role() {
        let roles_with_requester = vec!["Requester".to_string(), "Team Admin".to_string()];
        let roles_without_requester = vec!["Approver".to_string(), "Team Admin".to_string()];
        let empty_roles = vec![];

        assert!(RoleAuthorizer::has_requester_role(&roles_with_requester));
        assert!(!RoleAuthorizer::has_requester_role(
            &roles_without_requester
        ));
        assert!(!RoleAuthorizer::has_requester_role(&empty_roles));
    }

    #[test]
    fn test_has_approver_role() {
        let roles_with_approver = vec!["Approver".to_string(), "Team Admin".to_string()];
        let roles_without_approver = vec!["Requester".to_string(), "Team Admin".to_string()];
        let empty_roles = vec![];

        assert!(RoleAuthorizer::has_approver_role(&roles_with_approver));
        assert!(!RoleAuthorizer::has_approver_role(&roles_without_approver));
        assert!(!RoleAuthorizer::has_approver_role(&empty_roles));
    }

    #[test]
    fn test_authorize_stage_change_request_requester_operations() {
        let requester_roles = vec!["Requester".to_string()];
        let approver_roles = vec!["Approver".to_string()];
        let no_roles = vec![];

        // Test deployment request - requires Requester role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(
                &requester_roles,
                "DEPLOYMENT_REQUESTED"
            )
            .is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "DEPLOYMENT_REQUESTED")
                .is_err()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&no_roles, "DEPLOYMENT_REQUESTED")
                .is_err()
        );

        // Test rollback request - requires Requester role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "ROLLBACK_REQUESTED")
                .is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "ROLLBACK_REQUESTED")
                .is_err()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&no_roles, "ROLLBACK_REQUESTED")
                .is_err()
        );
    }

    #[test]
    fn test_authorize_stage_change_request_approver_operations() {
        let requester_roles = vec!["Requester".to_string()];
        let approver_roles = vec!["Approver".to_string()];
        let no_roles = vec![];

        // Test deployment execution - requires Requester or Approver role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "DEPLOYED").is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "DEPLOYED").is_ok()
        );
        assert!(RoleAuthorizer::authorize_stage_change_request(&no_roles, "DEPLOYED").is_err());

        // Test deployment rejection - requires Approver role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "DEPLOYMENT_REJECTED")
                .is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "DEPLOYMENT_REJECTED")
                .is_err()
        );

        // Test rollback execution - requires Requester or Approver role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "ROLLBACKED").is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "ROLLBACKED").is_ok()
        );

        // Test rollback rejection - requires Approver role
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&approver_roles, "ROLLBACK_REJECTED")
                .is_ok()
        );
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "ROLLBACK_REJECTED")
                .is_err()
        );
    }

    #[test]
    fn test_authorize_stage_change_request_with_multiple_roles() {
        let both_roles = vec!["Requester".to_string(), "Approver".to_string()];

        // User with both roles should be able to perform all operations
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&both_roles, "DEPLOYMENT_REQUESTED")
                .is_ok()
        );
        assert!(RoleAuthorizer::authorize_stage_change_request(&both_roles, "DEPLOYED").is_ok());
        assert!(
            RoleAuthorizer::authorize_stage_change_request(&both_roles, "ROLLBACK_REQUESTED")
                .is_ok()
        );
        assert!(RoleAuthorizer::authorize_stage_change_request(&both_roles, "ROLLBACKED").is_ok());
    }

    #[test]
    fn test_authorize_stage_change_request_unknown_operation() {
        let requester_roles = vec!["Requester".to_string()];

        assert!(
            RoleAuthorizer::authorize_stage_change_request(&requester_roles, "UNKNOWN_OPERATION")
                .is_err()
        );
    }
}
