//! Role, team and admin synchronisation from IdP group claims.
//!
//! Called inside the SSO callback transaction after the user is resolved. Groups come
//! from the provider's `groups_claim` (a dot path into the id_token claims, with the
//! userinfo claims as fallback) and are matched exactly against the provider's group
//! mappings.
//!
//! Modes: `off` does nothing; `additive` only adds SSO-sourced roles and teams and
//! grants admin; `authoritative` also removes SSO-sourced roles and teams that no
//! group maps to, and revokes admin that SSO granted earlier. Manual assignments and
//! manually granted admin are never touched.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::entity::SsoProvider;
use crate::database::role::RoleRepositoryTx;
use crate::database::sso_group_mapping::SsoGroupMappingRepositoryTx;
use crate::database::user::UserRepositoryTx;
use crate::logic::user_tx::ensure_another_admin_remains;
use crate::utils::activity_logger::{activity_types, entity_types};
use serde_json::Value;
use sqlx::PgConnection;
use std::collections::BTreeSet;
use uuid::Uuid;

/// Repositories used by [`sync_roles_from_claims`].
pub struct SyncRepos<'a, U, R, M> {
    pub users: &'a U,
    pub roles: &'a R,
    pub mappings: &'a M,
    pub activity: &'a dyn ActivityLogRepository,
}

/// Looks up `path` in `claims`: first as a literal key (URL-style claim names contain
/// dots), then as a dot-separated path through nested objects.
pub fn claim_at_path<'a>(claims: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    if let Some(value) = claims.get(path) {
        return Some(value);
    }
    let mut current = claims;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Whether the id_token carries Entra's group overage marker: the groups were too many
/// for the token and must be fetched from Graph, which FluxGate does not do.
pub fn has_group_overage(claims: &Value) -> bool {
    claims
        .get("_claim_names")
        .and_then(|names| names.get("groups"))
        .is_some()
}

/// Group values for the user. A string is one group, an array yields its string
/// elements, anything else is no groups. The userinfo claims are only consulted when
/// the id_token has no such claim at all. The overage marker means no groups.
pub fn extract_groups(
    groups_claim: &str,
    id_token_claims: &Value,
    userinfo: Option<&Value>,
) -> BTreeSet<String> {
    let found = match claim_at_path(id_token_claims, groups_claim) {
        Some(value) => Some(value),
        None if has_group_overage(id_token_claims) => {
            log::warn!(
                "SSO id_token has a group overage marker; groups are not synced for this login"
            );
            return BTreeSet::new();
        }
        None => userinfo.and_then(|info| claim_at_path(info, groups_claim)),
    };
    match found {
        Some(Value::String(group)) => BTreeSet::from([group.clone()]),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => BTreeSet::new(),
    }
}

/// What the user's groups ask for, before checking that the targets still exist.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DesiredAssignments {
    pub roles: BTreeSet<Uuid>,
    pub teams: BTreeSet<Uuid>,
    pub admin: bool,
}

pub fn desired_assignments(
    mappings: &[crate::database::entity::SsoGroupMapping],
    groups: &BTreeSet<String>,
) -> DesiredAssignments {
    let mut desired = DesiredAssignments::default();
    for mapping in mappings.iter().filter(|m| groups.contains(&m.group_value)) {
        match (mapping.target_type.as_str(), mapping.target_id) {
            ("role", Some(id)) => {
                desired.roles.insert(id);
            }
            ("team", Some(id)) => {
                desired.teams.insert(id);
            }
            ("admin", _) => desired.admin = true,
            _ => {}
        }
    }
    desired
}

/// Applies the provider's group mappings to the user. `id_token_claims` holds every
/// id_token claim; `userinfo` the userinfo claims when they were fetched.
pub async fn sync_roles_from_claims<U, R, M>(
    conn: &mut PgConnection,
    repos: SyncRepos<'_, U, R, M>,
    provider: &SsoProvider,
    user_id: Uuid,
    id_token_claims: &Value,
    userinfo: Option<&Value>,
) -> Result<(), Error>
where
    U: UserRepositoryTx,
    R: RoleRepositoryTx,
    M: SsoGroupMappingRepositoryTx,
{
    let authoritative = match provider.role_sync_mode.as_str() {
        "authoritative" => true,
        "additive" => false,
        _ => return Ok(()),
    };

    let groups = extract_groups(&provider.groups_claim, id_token_claims, userinfo);
    let mappings = repos.mappings.list_mappings_tx(conn, provider.id).await?;
    let desired = desired_assignments(&mappings, &groups);

    // A mapping can outlive its target only through a race; skip such targets.
    let desired_roles: BTreeSet<Uuid> = repos
        .roles
        .existing_role_ids_tx(conn, desired.roles.iter().copied().collect())
        .await?
        .into_iter()
        .collect();
    let desired_teams: BTreeSet<Uuid> = repos
        .users
        .existing_team_ids_tx(conn, desired.teams.iter().copied().collect())
        .await?
        .into_iter()
        .collect();

    let current_roles: BTreeSet<Uuid> = repos
        .roles
        .list_sso_role_ids_tx(conn, user_id)
        .await?
        .into_iter()
        .collect();
    let current_teams: BTreeSet<Uuid> = repos
        .users
        .list_sso_team_ids_tx(conn, user_id)
        .await?
        .into_iter()
        .collect();

    let add_roles: Vec<Uuid> = desired_roles.difference(&current_roles).copied().collect();
    let add_teams: Vec<Uuid> = desired_teams.difference(&current_teams).copied().collect();
    let remove_roles: Vec<Uuid> = if authoritative {
        current_roles.difference(&desired_roles).copied().collect()
    } else {
        Vec::new()
    };
    let remove_teams: Vec<Uuid> = if authoritative {
        current_teams.difference(&desired_teams).copied().collect()
    } else {
        Vec::new()
    };

    // The add call keeps an existing manual row manual, so "added" can include a
    // role the user already holds manually; report only what is new to the user.
    if !add_roles.is_empty() {
        repos
            .roles
            .add_sso_user_roles_tx(conn, user_id, add_roles.clone())
            .await?;
    }
    if !add_teams.is_empty() {
        repos
            .users
            .add_sso_user_teams_tx(conn, user_id, add_teams.clone())
            .await?;
    }
    if !remove_roles.is_empty() {
        repos
            .roles
            .remove_sso_user_roles_tx(conn, user_id, remove_roles.clone())
            .await?;
    }
    if !remove_teams.is_empty() {
        repos
            .users
            .remove_sso_user_teams_tx(conn, user_id, remove_teams.clone())
            .await?;
    }

    let (is_admin, admin_source) = repos.users.get_admin_state_tx(conn, user_id).await?;
    let mut admin_change: Option<&str> = None;
    let mut admin_kept_warning = false;
    if desired.admin {
        if !is_admin {
            repos
                .users
                .set_admin_with_source_tx(conn, user_id, true, Some("sso"))
                .await?;
            admin_change = Some("granted");
        }
    } else if authoritative && is_admin && admin_source.as_deref() == Some("sso") {
        match ensure_another_admin_remains(conn, user_id).await {
            Ok(()) => {
                repos
                    .users
                    .set_admin_with_source_tx(conn, user_id, false, None)
                    .await?;
                admin_change = Some("revoked");
            }
            Err(Error::LastAdminRequired) => {
                log::warn!(
                    "SSO sync kept admin for user {user_id}: no other enabled admin would remain"
                );
                admin_kept_warning = true;
            }
            Err(err) => return Err(err),
        }
    }

    let ids = |values: &[Uuid]| values.iter().map(Uuid::to_string).collect::<Vec<_>>();
    let changed = !add_roles.is_empty()
        || !add_teams.is_empty()
        || !remove_roles.is_empty()
        || !remove_teams.is_empty()
        || admin_change.is_some();
    let metadata = |extra: Option<(&str, Value)>| {
        let mut meta = serde_json::json!({
            "provider_id": provider.id.to_string(),
            "provider_slug": provider.slug,
            "added_roles": ids(&add_roles),
            "removed_roles": ids(&remove_roles),
            "added_teams": ids(&add_teams),
            "removed_teams": ids(&remove_teams),
            "admin_change": admin_change,
        });
        if let (Some((key, value)), Some(map)) = (extra, meta.as_object_mut()) {
            map.insert(key.to_string(), value);
        }
        meta
    };

    if changed {
        log_sync(
            conn,
            repos.activity,
            user_id,
            activity_types::SSO_ROLE_SYNC,
            "SSO group sync changed roles, teams or admin".to_string(),
            metadata(None),
        )
        .await?;
    }
    if admin_kept_warning {
        log_sync(
            conn,
            repos.activity,
            user_id,
            activity_types::SSO_ROLE_SYNC_WARNING,
            "SSO group sync kept admin: last enabled admin".to_string(),
            metadata(Some(("reason", Value::String("last_admin".to_string())))),
        )
        .await?;
    }
    Ok(())
}

async fn log_sync(
    conn: &mut PgConnection,
    activity: &dyn ActivityLogRepository,
    user_id: Uuid,
    activity_type: &str,
    description: String,
    metadata: Value,
) -> Result<(), Error> {
    activity
        .create_activity_tx(
            conn,
            CreateActivityLog {
                activity_type: activity_type.to_string(),
                entity_type: entity_types::USER.to_string(),
                entity_id: user_id.to_string(),
                actor_id: Some(user_id),
                actor_name: None,
                description,
                metadata: Some(metadata),
            },
        )
        .await
        .map_err(Error::DatabaseError)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::entity::SsoGroupMapping;
    use serde_json::json;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn flat_array_claim() {
        let claims = json!({"groups": ["a", "b"]});
        assert_eq!(extract_groups("groups", &claims, None), set(&["a", "b"]));
    }

    #[test]
    fn nested_claim_path() {
        let claims = json!({"realm_access": {"roles": ["admins", "devs"]}});
        assert_eq!(
            extract_groups("realm_access.roles", &claims, None),
            set(&["admins", "devs"])
        );
    }

    #[test]
    fn literal_key_with_dots_wins_over_path() {
        let claims =
            json!({"https://idp.example/groups": ["x"], "https://idp": {"example/groups": ["y"]}});
        assert_eq!(
            extract_groups("https://idp.example/groups", &claims, None),
            set(&["x"])
        );
    }

    #[test]
    fn string_claim_is_one_group() {
        let claims = json!({"groups": "solo"});
        assert_eq!(extract_groups("groups", &claims, None), set(&["solo"]));
    }

    #[test]
    fn missing_or_wrongly_typed_claim_is_no_groups() {
        assert!(extract_groups("groups", &json!({}), None).is_empty());
        assert!(extract_groups("groups", &json!({"groups": 5}), None).is_empty());
        assert!(extract_groups("groups", &json!({"groups": {"a": 1}}), None).is_empty());
        assert!(extract_groups("groups", &json!({"groups": null}), None).is_empty());
        assert!(extract_groups("", &json!({"groups": ["a"]}), None).is_empty());
        assert_eq!(
            extract_groups("groups", &json!({"groups": ["a", 1, null, "b"]}), None),
            set(&["a", "b"])
        );
    }

    #[test]
    fn wrongly_typed_id_token_claim_does_not_fall_back_to_userinfo() {
        let userinfo = json!({"groups": ["from-userinfo"]});
        assert!(extract_groups("groups", &json!({"groups": 5}), Some(&userinfo)).is_empty());
    }

    #[test]
    fn userinfo_is_used_only_when_id_token_has_no_claim() {
        let userinfo = json!({"groups": ["from-userinfo"]});
        assert_eq!(
            extract_groups("groups", &json!({}), Some(&userinfo)),
            set(&["from-userinfo"])
        );
        assert_eq!(
            extract_groups("groups", &json!({"groups": ["token"]}), Some(&userinfo)),
            set(&["token"])
        );
    }

    #[test]
    fn overage_marker_means_no_groups() {
        let claims = json!({"_claim_names": {"groups": "src1"}, "_claim_sources": {"src1": {}}});
        let userinfo = json!({"groups": ["from-userinfo"]});
        assert!(has_group_overage(&claims));
        assert!(extract_groups("groups", &claims, Some(&userinfo)).is_empty());
        assert!(!has_group_overage(&json!({"_claim_names": {"other": "x"}})));
    }

    #[test]
    fn mappings_match_exactly_and_case_sensitively() {
        let (role, team) = (Uuid::new_v4(), Uuid::new_v4());
        let mapping = |group: &str, kind: &str, target: Option<Uuid>| SsoGroupMapping {
            id: Uuid::new_v4(),
            provider_id: Uuid::nil(),
            group_value: group.to_string(),
            target_type: kind.to_string(),
            target_id: target,
        };
        let mappings = vec![
            mapping("Devs", "role", Some(role)),
            mapping("Devs", "team", Some(team)),
            mapping("devs", "admin", None),
        ];
        let desired = desired_assignments(&mappings, &set(&["Devs"]));
        assert_eq!(desired.roles, BTreeSet::from([role]));
        assert_eq!(desired.teams, BTreeSet::from([team]));
        assert!(!desired.admin);
        let desired = desired_assignments(&mappings, &set(&["devs", "DEVS"]));
        assert_eq!(
            desired,
            DesiredAssignments {
                admin: true,
                ..Default::default()
            }
        );
    }
}
