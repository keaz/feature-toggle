//! Persistence for TypeSafe judgments and per-team AI toggles.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mockall::automock;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::database::{Error, handle_error};
use crate::judgment::{JudgmentKind, SubjectType};

/// Most runs of one input that may reach the TypeSafe API. `attempts` counts
/// runs that reached the API: a run reserves an attempt (`start_attempt`) right
/// before its API call and gives it back (`refund_attempt`) when the HTTP client
/// had no free request slot and sent nothing.
pub const MAX_ATTEMPTS: i32 = 3;
/// Most times the retry sweep picks up one input. Bounds a row whose runs never
/// get an API permit (and so never spend an attempt): with the 2-minute claim
/// spacing it is retried for at least 20 minutes, then left alone.
pub const MAX_CLAIMS: i32 = 10;
/// Rows the retry sweep takes per tick.
pub const RETRY_BATCH: i64 = 50;

const TEAM_FK_CONSTRAINT: &str = "team_ai_settings_team_id_fkey";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFeature {
    ApprovalRisk,
    JustificationCheck,
    FlagKind,
    NlSearch,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TeamAiSettings {
    pub approval_risk: bool,
    pub justification_check: bool,
    pub flag_kind: bool,
    pub nl_search: bool,
}

impl TeamAiSettings {
    pub fn is_enabled(&self, feature: AiFeature) -> bool {
        match feature {
            AiFeature::ApprovalRisk => self.approval_risk,
            AiFeature::JustificationCheck => self.justification_check,
            AiFeature::FlagKind => self.flag_kind,
            AiFeature::NlSearch => self.nl_search,
        }
    }
}

/// Settings plus audit fields. `updated_at` is `None` when no row exists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredTeamAiSettings {
    pub settings: TeamAiSettings,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Option<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct TeamAiSettingsRow {
    approval_risk: bool,
    justification_check: bool,
    flag_kind: bool,
    nl_search: bool,
    updated_at: DateTime<Utc>,
    updated_by: Option<Uuid>,
}

impl From<TeamAiSettingsRow> for StoredTeamAiSettings {
    fn from(row: TeamAiSettingsRow) -> Self {
        Self {
            settings: TeamAiSettings {
                approval_risk: row.approval_risk,
                justification_check: row.justification_check,
                flag_kind: row.flag_kind,
                nl_search: row.nl_search,
            },
            updated_at: Some(row.updated_at),
            updated_by: row.updated_by,
        }
    }
}

#[automock]
#[async_trait]
pub trait TeamAiSettingsRepository: Send + Sync {
    /// All features off when the team has no row.
    async fn get(&self, team_id: Uuid) -> Result<StoredTeamAiSettings, Error>;
    /// `Error::NotFound(team_id)` when the team does not exist.
    async fn upsert(
        &self,
        team_id: Uuid,
        settings: TeamAiSettings,
        updated_by: Option<Uuid>,
    ) -> Result<StoredTeamAiSettings, Error>;
    fn clone_box(&self) -> Box<dyn TeamAiSettingsRepository>;
}

impl Clone for Box<dyn TeamAiSettingsRepository> {
    fn clone(&self) -> Box<dyn TeamAiSettingsRepository> {
        self.clone_box()
    }
}

pub fn team_ai_settings_repository(pool: PgPool) -> Box<dyn TeamAiSettingsRepository> {
    Box::new(PgTeamAiSettingsRepository { pool })
}

#[derive(Clone)]
pub struct PgTeamAiSettingsRepository {
    pool: PgPool,
}

const SETTINGS_COLUMNS: &str =
    "approval_risk, justification_check, flag_kind, nl_search, updated_at, updated_by";

#[async_trait]
impl TeamAiSettingsRepository for PgTeamAiSettingsRepository {
    async fn get(&self, team_id: Uuid) -> Result<StoredTeamAiSettings, Error> {
        let result = sqlx::query_as::<_, TeamAiSettingsRow>(&format!(
            "SELECT {SETTINGS_COLUMNS} FROM team_ai_settings WHERE team_id = $1"
        ))
        .bind(team_id)
        .fetch_optional(&self.pool)
        .await;
        Ok(handle_error(None, result)?
            .map(StoredTeamAiSettings::from)
            .unwrap_or_default())
    }

    async fn upsert(
        &self,
        team_id: Uuid,
        settings: TeamAiSettings,
        updated_by: Option<Uuid>,
    ) -> Result<StoredTeamAiSettings, Error> {
        let result = sqlx::query_as::<_, TeamAiSettingsRow>(&format!(
            r#"
            INSERT INTO team_ai_settings
                (team_id, approval_risk, justification_check, flag_kind, nl_search, updated_at, updated_by)
            VALUES ($1, $2, $3, $4, $5, NOW(), $6)
            ON CONFLICT (team_id) DO UPDATE SET
                approval_risk = EXCLUDED.approval_risk,
                justification_check = EXCLUDED.justification_check,
                flag_kind = EXCLUDED.flag_kind,
                nl_search = EXCLUDED.nl_search,
                updated_at = NOW(),
                updated_by = EXCLUDED.updated_by
            RETURNING {SETTINGS_COLUMNS}
            "#
        ))
        .bind(team_id)
        .bind(settings.approval_risk)
        .bind(settings.justification_check)
        .bind(settings.flag_kind)
        .bind(settings.nl_search)
        .bind(updated_by)
        .fetch_one(&self.pool)
        .await;

        match result {
            Err(sqlx::Error::Database(db_error))
                if db_error.constraint() == Some(TEAM_FK_CONSTRAINT) =>
            {
                Err(Error::NotFound(team_id))
            }
            other => handle_error(Some(team_id), other).map(StoredTeamAiSettings::from),
        }
    }

    fn clone_box(&self) -> Box<dyn TeamAiSettingsRepository> {
        Box::new(self.clone())
    }
}

/// One row of `ai_judgments`. `kind`, `subject_type`, `status` are the stored strings.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AiJudgment {
    pub id: Uuid,
    pub team_id: Uuid,
    pub subject_type: String,
    pub subject_id: Uuid,
    pub kind: String,
    pub status: String,
    pub attempts: i32,
    pub input: Value,
    pub input_hash: String,
    pub model: Option<String>,
    pub raw_answers: Option<Value>,
    pub derived: Option<Value>,
    pub input_tokens: Option<i32>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewJudgment {
    pub team_id: Uuid,
    pub kind: JudgmentKind,
    pub subject_type: SubjectType,
    pub subject_id: Uuid,
    pub input: Value,
    pub input_hash: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JudgmentResult {
    pub model: String,
    pub raw_answers: Value,
    pub derived: Value,
    pub input_tokens: Option<i32>,
}

#[automock]
#[async_trait]
pub trait AiJudgmentRepository: Send + Sync {
    /// Inserts or resets the row for (subject_type, subject_id, kind) to `pending`
    /// with `attempts = 0` and no sweep claims: nothing has reached the API yet.
    async fn upsert_pending(&self, judgment: NewJudgment) -> Result<AiJudgment, Error>;
    /// Reserves one API attempt right before a run calls the API. False (and no
    /// call should be made) when `input_hash` no longer matches, the row is
    /// done or skipped, or `MAX_ATTEMPTS` runs already reached the API. Atomic, so
    /// concurrent runs on several nodes cannot exceed the bound.
    async fn start_attempt(&self, id: Uuid, input_hash: String) -> Result<bool, Error>;
    /// Gives back an attempt reserved by `start_attempt` when the run sent
    /// nothing (no free API request slot). False when the hash is stale or no
    /// attempt is counted.
    async fn refund_attempt(&self, id: Uuid, input_hash: String) -> Result<bool, Error>;
    /// False when `input_hash` no longer matches (a newer submission won), the
    /// row is already done (another run of the same input finished first), or
    /// the row was skipped while the run was in flight.
    async fn mark_done(
        &self,
        id: Uuid,
        input_hash: String,
        result: JudgmentResult,
    ) -> Result<bool, Error>;
    /// False when the hash is stale or the row is already done or skipped. Does
    /// not count an attempt: a run counts it when it reaches the API
    /// (`start_attempt`).
    async fn mark_failed(&self, id: Uuid, input_hash: String, error: String)
    -> Result<bool, Error>;
    /// Closes the subject's unfinished (`pending` or `failed`) row for good:
    /// `skipped`, with `reason` as the error. A skipped row is never started,
    /// finished, failed or retried. False when there is no unfinished row (none
    /// at all, or it is already done or skipped).
    async fn skip_unfinished(
        &self,
        subject_type: SubjectType,
        subject_id: Uuid,
        kind: JudgmentKind,
        reason: String,
    ) -> Result<bool, Error>;
    async fn get_for_subject(
        &self,
        subject_type: SubjectType,
        subject_id: Uuid,
        kind: JudgmentKind,
    ) -> Result<Option<AiJudgment>, Error>;
    async fn get_for_subjects(
        &self,
        subject_type: SubjectType,
        subject_ids: Vec<Uuid>,
        kind: JudgmentKind,
    ) -> Result<Vec<AiJudgment>, Error>;
    /// Claims rows for the retry sweep, oldest first: pending rows older than
    /// 2 minutes (their run was lost, deferred, or never stored) and failed
    /// rows, both only while `attempts < MAX_ATTEMPTS` and
    /// `claims < MAX_CLAIMS`, and only 2 minutes after the row's last claim (so
    /// a run in progress is not picked up again). Claiming counts a claim, not
    /// an attempt.
    async fn claim_retryable(&self, limit: i64) -> Result<Vec<AiJudgment>, Error>;
    fn clone_box(&self) -> Box<dyn AiJudgmentRepository>;
}

impl Clone for Box<dyn AiJudgmentRepository> {
    fn clone(&self) -> Box<dyn AiJudgmentRepository> {
        self.clone_box()
    }
}

pub fn ai_judgment_repository(pool: PgPool) -> Box<dyn AiJudgmentRepository> {
    Box::new(PgAiJudgmentRepository { pool })
}

#[derive(Clone)]
pub struct PgAiJudgmentRepository {
    pool: PgPool,
}

const JUDGMENT_COLUMNS: &str = "id, team_id, subject_type, subject_id, kind, status, attempts, \
     input, input_hash, model, raw_answers, derived, input_tokens, error, created_at, completed_at";

#[async_trait]
impl AiJudgmentRepository for PgAiJudgmentRepository {
    async fn upsert_pending(&self, judgment: NewJudgment) -> Result<AiJudgment, Error> {
        let result = sqlx::query_as::<_, AiJudgment>(&format!(
            r#"
            INSERT INTO ai_judgments
                (id, team_id, subject_type, subject_id, kind, status, attempts, input, input_hash)
            VALUES ($1, $2, $3, $4, $5, 'pending', 0, $6, $7)
            ON CONFLICT (subject_type, subject_id, kind) DO UPDATE SET
                team_id = EXCLUDED.team_id,
                status = 'pending',
                attempts = 0,
                claims = 0,
                claimed_at = NULL,
                input = EXCLUDED.input,
                input_hash = EXCLUDED.input_hash,
                model = NULL,
                raw_answers = NULL,
                derived = NULL,
                input_tokens = NULL,
                error = NULL,
                created_at = NOW(),
                completed_at = NULL
            RETURNING {JUDGMENT_COLUMNS}
            "#
        ))
        .bind(Uuid::new_v4())
        .bind(judgment.team_id)
        .bind(judgment.subject_type.as_str())
        .bind(judgment.subject_id)
        .bind(judgment.kind.as_str())
        .bind(&judgment.input)
        .bind(&judgment.input_hash)
        .fetch_one(&self.pool)
        .await;
        handle_error(None, result)
    }

    async fn mark_done(
        &self,
        id: Uuid,
        input_hash: String,
        result: JudgmentResult,
    ) -> Result<bool, Error> {
        let outcome = sqlx::query(
            r#"
            UPDATE ai_judgments
            SET status = 'done', model = $3, raw_answers = $4, derived = $5,
                input_tokens = $6, error = NULL, completed_at = NOW()
            WHERE id = $1 AND input_hash = $2 AND status IN ('pending', 'failed')
            "#,
        )
        .bind(id)
        .bind(&input_hash)
        .bind(&result.model)
        .bind(&result.raw_answers)
        .bind(&result.derived)
        .bind(result.input_tokens)
        .execute(&self.pool)
        .await;
        Ok(handle_error(Some(id), outcome)?.rows_affected() > 0)
    }

    async fn start_attempt(&self, id: Uuid, input_hash: String) -> Result<bool, Error> {
        let outcome = sqlx::query(
            r#"
            UPDATE ai_judgments SET attempts = attempts + 1
            WHERE id = $1 AND input_hash = $2 AND status IN ('pending', 'failed')
              AND attempts < $3
            "#,
        )
        .bind(id)
        .bind(&input_hash)
        .bind(MAX_ATTEMPTS)
        .execute(&self.pool)
        .await;
        Ok(handle_error(Some(id), outcome)?.rows_affected() > 0)
    }

    async fn refund_attempt(&self, id: Uuid, input_hash: String) -> Result<bool, Error> {
        let outcome = sqlx::query(
            r#"
            UPDATE ai_judgments SET attempts = attempts - 1
            WHERE id = $1 AND input_hash = $2 AND attempts > 0
            "#,
        )
        .bind(id)
        .bind(&input_hash)
        .execute(&self.pool)
        .await;
        Ok(handle_error(Some(id), outcome)?.rows_affected() > 0)
    }

    async fn mark_failed(
        &self,
        id: Uuid,
        input_hash: String,
        error: String,
    ) -> Result<bool, Error> {
        let outcome = sqlx::query(
            r#"
            UPDATE ai_judgments
            SET status = 'failed', error = $3
            WHERE id = $1 AND input_hash = $2 AND status IN ('pending', 'failed')
            "#,
        )
        .bind(id)
        .bind(&input_hash)
        .bind(&error)
        .execute(&self.pool)
        .await;
        Ok(handle_error(Some(id), outcome)?.rows_affected() > 0)
    }

    async fn skip_unfinished(
        &self,
        subject_type: SubjectType,
        subject_id: Uuid,
        kind: JudgmentKind,
        reason: String,
    ) -> Result<bool, Error> {
        let outcome = sqlx::query(
            r#"
            UPDATE ai_judgments
            SET status = 'skipped', error = $4, completed_at = NOW()
            WHERE subject_type = $1 AND subject_id = $2 AND kind = $3
              AND status IN ('pending', 'failed')
            "#,
        )
        .bind(subject_type.as_str())
        .bind(subject_id)
        .bind(kind.as_str())
        .bind(&reason)
        .execute(&self.pool)
        .await;
        Ok(handle_error(None, outcome)?.rows_affected() > 0)
    }

    async fn get_for_subject(
        &self,
        subject_type: SubjectType,
        subject_id: Uuid,
        kind: JudgmentKind,
    ) -> Result<Option<AiJudgment>, Error> {
        let result = sqlx::query_as::<_, AiJudgment>(&format!(
            "SELECT {JUDGMENT_COLUMNS} FROM ai_judgments \
             WHERE subject_type = $1 AND subject_id = $2 AND kind = $3"
        ))
        .bind(subject_type.as_str())
        .bind(subject_id)
        .bind(kind.as_str())
        .fetch_optional(&self.pool)
        .await;
        handle_error(None, result)
    }

    async fn get_for_subjects(
        &self,
        subject_type: SubjectType,
        subject_ids: Vec<Uuid>,
        kind: JudgmentKind,
    ) -> Result<Vec<AiJudgment>, Error> {
        if subject_ids.is_empty() {
            return Ok(Vec::new());
        }
        let result = sqlx::query_as::<_, AiJudgment>(&format!(
            "SELECT {JUDGMENT_COLUMNS} FROM ai_judgments \
             WHERE subject_type = $1 AND subject_id = ANY($2) AND kind = $3"
        ))
        .bind(subject_type.as_str())
        .bind(&subject_ids)
        .bind(kind.as_str())
        .fetch_all(&self.pool)
        .await;
        handle_error(None, result)
    }

    async fn claim_retryable(&self, limit: i64) -> Result<Vec<AiJudgment>, Error> {
        let result = sqlx::query_as::<_, AiJudgment>(&format!(
            r#"
            UPDATE ai_judgments SET claims = claims + 1, claimed_at = NOW()
            WHERE id IN (
                SELECT id FROM ai_judgments
                WHERE ((status = 'pending' AND created_at < NOW() - INTERVAL '2 minutes')
                       OR status = 'failed')
                  AND attempts < $1
                  AND claims < $2
                  AND (claimed_at IS NULL OR claimed_at < NOW() - INTERVAL '2 minutes')
                ORDER BY created_at ASC
                LIMIT $3
                FOR UPDATE SKIP LOCKED
            )
            RETURNING {JUDGMENT_COLUMNS}
            "#
        ))
        .bind(MAX_ATTEMPTS)
        .bind(MAX_CLAIMS)
        .bind(limit)
        .fetch_all(&self.pool)
        .await;
        let mut rows = handle_error(None, result)?;
        rows.sort_by_key(|row| row.created_at);
        Ok(rows)
    }

    fn clone_box(&self) -> Box<dyn AiJudgmentRepository> {
        Box::new(self.clone())
    }
}
