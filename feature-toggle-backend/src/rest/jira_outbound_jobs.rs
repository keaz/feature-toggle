//! The Jira write-back job list and retry (JI-42), behind the same policy as the rest
//! of `/jira-integrations/**` (team admin or admin; system clients are denied).

use actix_web::{HttpResponse, Responder, get, post, web};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::entity::{JiraIntegrationRow, OutboundJobRow};
use crate::database::jira_integration::JiraIntegrationRepository;
use crate::database::jira_outbound_job::JiraOutboundJobRepository;
use crate::rest::error::RestError;
use crate::rest::pagination::{DEFAULT_LIMIT, MAX_LIMIT};

const STATUSES: [&str; 3] = ["pending", "sent", "dead"];

/// One write-back job. The payload is not returned.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraOutboundJobResponse {
    pub id: String,
    pub issue_key: String,
    /// `None` after the feature was deleted.
    pub feature_id: Option<String>,
    /// `comment`, `remote_link` or `remote_link_delete`.
    pub kind: String,
    /// `pending`, `sent` or `dead`.
    pub status: String,
    pub attempts: i32,
    pub next_attempt_at: DateTime<Utc>,
    /// Last Jira status and the start of its answer, or why the job was dropped.
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JiraOutboundJobsResponse {
    pub items: Vec<JiraOutboundJobResponse>,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
pub struct OutboundJobsQuery {
    pub status: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

fn map_job(row: OutboundJobRow) -> JiraOutboundJobResponse {
    JiraOutboundJobResponse {
        id: row.id.to_string(),
        issue_key: row.issue_key,
        feature_id: row.feature_id.map(|id| id.to_string()),
        kind: row.kind,
        status: row.status,
        attempts: row.attempts,
        next_attempt_at: row.next_attempt_at,
        last_error: row.last_error,
        created_at: row.created_at,
        sent_at: row.sent_at,
    }
}

fn parse_uuid(value: &str, what: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {what}")))
}

async fn ensure_integration(
    integrations: &dyn JiraIntegrationRepository,
    id: Uuid,
) -> Result<JiraIntegrationRow, RestError> {
    integrations
        .get(id)
        .await?
        .ok_or_else(|| RestError::not_found(format!("Jira integration {id} not found")))
}

#[utoipa::path(
    get,
    path = "/api/v1/jira-integrations/{id}/outbound-jobs",
    params(
        ("id" = String, Path, description = "Jira integration ID"),
        ("status" = Option<String>, Query, description = "Only jobs with this status: pending, sent or dead"),
        ("offset" = Option<i64>, Query, description = "Jobs to skip"),
        ("limit" = Option<i64>, Query, description = "Jobs to return (default 50, at most 200)")
    ),
    responses(
        (status = 200, description = "Write-back jobs of the integration, newest first", body = JiraOutboundJobsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[get("/jira-integrations/{id}/outbound-jobs")]
pub(crate) async fn list_jira_outbound_jobs(
    integrations: web::Data<Box<dyn JiraIntegrationRepository>>,
    jobs: web::Data<Box<dyn JiraOutboundJobRepository>>,
    id: web::Path<String>,
    query: web::Query<OutboundJobsQuery>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let status = match query.status.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(status) if STATUSES.contains(&status) => Some(status.to_string()),
        Some(_) => {
            return Err(RestError::invalid_input(
                "status must be pending, sent or dead",
            ));
        }
    };
    ensure_integration(integrations.as_ref().as_ref(), id).await?;
    let offset = query.offset.unwrap_or(0).max(0);
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let (rows, total) = jobs.list(id, status, offset, limit).await?;
    Ok(HttpResponse::Ok().json(JiraOutboundJobsResponse {
        items: rows.into_iter().map(map_job).collect(),
        total,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/jira-integrations/{id}/outbound-jobs/{jobId}/retry",
    params(
        ("id" = String, Path, description = "Jira integration ID"),
        ("jobId" = String, Path, description = "Job ID")
    ),
    responses(
        (status = 200, description = "The job is pending again, with no attempts used", body = JiraOutboundJobResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "The job is not dead (or does not exist), a pending job already covers it, or write-back is off (integration disabled or write-back disabled)", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[post("/jira-integrations/{id}/outbound-jobs/{jobId}/retry")]
pub(crate) async fn retry_jira_outbound_job(
    integrations: web::Data<Box<dyn JiraIntegrationRepository>>,
    jobs: web::Data<Box<dyn JiraOutboundJobRepository>>,
    path: web::Path<(String, String)>,
) -> Result<impl Responder, RestError> {
    let (id, job_id) = path.into_inner();
    let id = parse_uuid(&id, "integration id")?;
    let job_id = parse_uuid(&job_id, "job id")?;
    let integration = ensure_integration(integrations.as_ref().as_ref(), id).await?;
    if !integration.enabled || !integration.writeback_enabled {
        // A revived job would be cancelled again (or wait forever): say so instead.
        return Err(RestError::conflict("write-back is off"));
    }
    let retried = jobs.retry_dead(id, job_id).await.map_err(|err| match err {
        // The partial unique index: a newer pending remote link refresh exists.
        crate::Error::RecordAlreadyExists(_) => {
            RestError::conflict("a pending job already covers this one")
        }
        other => RestError::from(other),
    })?;
    match retried {
        Some(row) => Ok(HttpResponse::Ok().json(map_job(row))),
        None => Err(RestError::conflict("job is not dead")),
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_jira_outbound_jobs)
        .service(retry_jira_outbound_job);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JwtUser;
    use crate::database::activity_log::activity_log_repository;
    use crate::database::init_pg_pool;
    use crate::database::jira_integration::{
        jira_integration_repository, jira_integration_repository_tx,
    };
    use crate::database::jira_outbound_job::{
        NewOutboundJob, OutboundKind, jira_outbound_job_repository,
    };
    use crate::logic::ActorContext;
    use crate::logic::jira_integration_tx::{JiraIntegrationInput, create_jira_integration_in_tx};
    use actix_web::{App, HttpMessage, http::StatusCode, test};
    use serial_test::serial;

    const SEED_ADMIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    struct Fixture {
        pool: sqlx::PgPool,
        team_id: Uuid,
        integration_id: Uuid,
        shadow_user: Uuid,
    }

    impl Fixture {
        async fn new() -> Self {
            let pool = init_pg_pool().await;
            let team_id = Uuid::new_v4();
            sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'jobs rest')")
                .bind(team_id)
                .bind(format!("jira-jobs-rest-{team_id}"))
                .execute(&pool)
                .await
                .expect("insert team");
            let repo = jira_integration_repository_tx(pool.clone());
            let activity = activity_log_repository(pool.clone());
            let mut tx = pool.begin().await.expect("begin");
            let created = create_jira_integration_in_tx(
                &mut tx,
                &repo,
                activity.as_ref(),
                team_id,
                JiraIntegrationInput {
                    name: "jobs".to_string(),
                    jira_base_url: None,
                    environment_field: "labels".to_string(),
                    environment_aliases: Default::default(),
                    jira_approved_environment_ids: vec![],
                    feature_key_field: None,
                    enabled: true,
                },
                ActorContext::new(Uuid::parse_str(SEED_ADMIN_ID).unwrap(), "admin".to_string()),
            )
            .await
            .expect("create integration");
            tx.commit().await.expect("commit");
            sqlx::query("UPDATE jira_integrations SET writeback_enabled = TRUE WHERE id = $1")
                .bind(created.integration.id)
                .execute(&pool)
                .await
                .expect("write-back on");
            Self {
                pool,
                team_id,
                integration_id: created.integration.id,
                shadow_user: created.integration.actor_user_id,
            }
        }

        async fn enqueue(&self, n: usize) -> Uuid {
            let key = format!("rest:{}:{n}", self.integration_id);
            jira_outbound_job_repository(self.pool.clone())
                .enqueue(NewOutboundJob {
                    integration_id: self.integration_id,
                    issue_key: format!("PROJ-{n}"),
                    feature_id: None,
                    kind: OutboundKind::Comment,
                    payload: serde_json::json!({"lines": ["secret payload text"]}),
                    dedupe_key: key.clone(),
                })
                .await
                .expect("enqueue");
            sqlx::query_scalar("SELECT id FROM jira_outbound_jobs WHERE dedupe_key = $1")
                .bind(key)
                .fetch_one(&self.pool)
                .await
                .expect("job id")
        }

        async fn call(&self, method: &str, uri: &str) -> (StatusCode, serde_json::Value) {
            let app = test::init_service(
                App::new()
                    .app_data(web::Data::new(jira_integration_repository(
                        self.pool.clone(),
                    )))
                    .app_data(web::Data::new(jira_outbound_job_repository(
                        self.pool.clone(),
                    )))
                    .service(web::scope("/api/v1").configure(super::configure)),
            )
            .await;
            let builder = if method == "GET" {
                test::TestRequest::get()
            } else {
                test::TestRequest::post()
            };
            let req = builder.uri(&format!("/api/v1{uri}")).to_request();
            req.extensions_mut().insert(JwtUser {
                id: Uuid::new_v4(),
                username: "admin".to_string(),
                is_admin: true,
                roles: Vec::new(),
                team_id: None,
                token_hash: "hash".to_string(),
            });
            let resp = test::call_service(&app, req).await;
            let status = resp.status();
            let bytes = test::read_body(resp).await;
            (status, serde_json::from_slice(&bytes).unwrap_or_default())
        }

        async fn cleanup(self) {
            sqlx::query("DELETE FROM teams WHERE id = $1")
                .bind(self.team_id)
                .execute(&self.pool)
                .await
                .expect("delete team");
            sqlx::query("DELETE FROM users WHERE id = $1")
                .bind(self.shadow_user)
                .execute(&self.pool)
                .await
                .expect("delete shadow user");
        }
    }

    #[actix_web::test]
    #[serial]
    async fn list_pages_newest_first_and_filters_by_status() {
        let fx = Fixture::new().await;
        let mut ids = Vec::new();
        for n in 1..=3 {
            ids.push(fx.enqueue(n).await);
        }
        jira_outbound_job_repository(fx.pool.clone())
            .mark_dead(ids[0], 6, "500: boom".to_string())
            .await
            .unwrap();
        let base = format!("/jira-integrations/{}/outbound-jobs", fx.integration_id);

        let (status, page) = fx.call("GET", &format!("{base}?limit=2")).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["total"], 3);
        assert_eq!(page["items"].as_array().map(Vec::len), Some(2));
        assert_eq!(page["items"][0]["id"], ids[2].to_string());
        assert_eq!(page["items"][0]["issueKey"], "PROJ-3");
        assert_eq!(page["items"][0]["kind"], "comment");
        assert_eq!(page["items"][0]["status"], "pending");
        assert!(page["items"][0].get("payload").is_none());
        assert!(!page.to_string().contains("secret payload text"));

        let (_, second) = fx.call("GET", &format!("{base}?limit=2&offset=2")).await;
        assert_eq!(second["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(second["items"][0]["id"], ids[0].to_string());

        let (_, dead) = fx.call("GET", &format!("{base}?status=dead")).await;
        assert_eq!(dead["total"], 1);
        assert_eq!(dead["items"][0]["lastError"], "500: boom");
        assert_eq!(dead["items"][0]["attempts"], 6);

        let (status, body) = fx.call("GET", &format!("{base}?status=bogus")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, body) = fx
            .call(
                "GET",
                &format!("/jira-integrations/{}/outbound-jobs", Uuid::new_v4()),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

        fx.cleanup().await;
    }

    #[actix_web::test]
    #[serial]
    async fn retry_revives_a_dead_job_once() {
        let fx = Fixture::new().await;
        let id = fx.enqueue(1).await;
        let uri = format!(
            "/jira-integrations/{}/outbound-jobs/{id}/retry",
            fx.integration_id
        );

        // Pending: not dead.
        let (status, body) = fx.call("POST", &uri).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["message"], "job is not dead");

        jira_outbound_job_repository(fx.pool.clone())
            .mark_dead(id, 6, "500: boom".to_string())
            .await
            .unwrap();
        let (status, job) = fx.call("POST", &uri).await;
        assert_eq!(status, StatusCode::OK, "{job}");
        assert_eq!(job["id"], id.to_string());
        assert_eq!(job["status"], "pending");
        assert_eq!(job["attempts"], 0);

        // Second retry: it is pending now.
        let (status, _) = fx.call("POST", &uri).await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (status, _) = fx
            .call(
                "POST",
                &format!(
                    "/jira-integrations/{}/outbound-jobs/not-a-uuid/retry",
                    fx.integration_id
                ),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        fx.cleanup().await;
    }

    #[actix_web::test]
    #[serial]
    async fn retry_is_409_while_the_integration_or_write_back_is_off() {
        let fx = Fixture::new().await;
        let id = fx.enqueue(1).await;
        jira_outbound_job_repository(fx.pool.clone())
            .mark_dead(id, 6, "500: boom".to_string())
            .await
            .unwrap();
        let uri = format!(
            "/jira-integrations/{}/outbound-jobs/{id}/retry",
            fx.integration_id
        );
        for (enabled, writeback) in [(true, false), (false, true)] {
            sqlx::query(
                "UPDATE jira_integrations SET enabled = $2, writeback_enabled = $3 WHERE id = $1",
            )
            .bind(fx.integration_id)
            .bind(enabled)
            .bind(writeback)
            .execute(&fx.pool)
            .await
            .unwrap();
            let (status, body) = fx.call("POST", &uri).await;
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            assert_eq!(body["message"], "write-back is off");
        }
        let (rows, _) = jira_outbound_job_repository(fx.pool.clone())
            .list(fx.integration_id, None, 0, 10)
            .await
            .unwrap();
        assert_eq!(rows[0].status, "dead", "the job was not revived");

        fx.cleanup().await;
    }
}
