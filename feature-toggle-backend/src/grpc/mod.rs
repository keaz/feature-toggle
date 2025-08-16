pub mod pb {
    tonic::include_proto!("featuretoggle");
}

use crate::database::client::client_repository;
use crate::database::entity as db;
use crate::database::feature::feature_repository;
use evaluation_engine as engine;
use pb::feature_evaluation_server::{FeatureEvaluation, FeatureEvaluationServer};
use pb::{EvaluateRequest, EvaluateResponse};
use tonic::{Request, Response, Status};
use uuid::Uuid;

pub use pb::feature_evaluation_server;
// re-export for server creation

#[derive(Clone)]
pub struct FeatureEvaluationSvc {
    pool: sqlx::PgPool,
}

impl FeatureEvaluationSvc {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    async fn map_db_feature_to_engine(&self, f: db::Feature) -> Result<engine::Feature, Status> {
        let repo = feature_repository(self.pool.clone());
        let mut stages = Vec::with_capacity(f.stages.len());
        for s in f.stages.into_iter() {
            // Load stage criterias
            let crits = repo
                .get_stage_criteria(s.id)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?;
            let mapped_criteria = crits
                .into_iter()
                .map(|c| engine::StageCriterion {
                    context_key: c.context_key,
                    context: engine::StageContext {
                        key: c.context.key,
                        entries: c.context.entries.into_iter().map(|e| e.value).collect(),
                    },
                    rollout_percentage: c.rollout_percentage,
                })
                .collect::<Vec<_>>();
            stages.push(engine::FeatureStage {
                environment_id: s.environment_id.to_string(),
                enabled: s.enabled,
                bucketing_key: s.bucketing_key,
                criterias: mapped_criteria,
            });
        }

        // Dependencies: load only as empty for now (requires recursive fetch if needed)
        let deps: Vec<engine::Feature> = vec![];

        Ok(engine::Feature {
            enabled: true, // top-level enablement not stored; treat as enabled
            dependencies: deps,
            stages,
        })
    }
}

#[tonic::async_trait]
impl FeatureEvaluation for FeatureEvaluationSvc {
    async fn evaluate(
        &self,
        request: Request<EvaluateRequest>,
    ) -> Result<Response<EvaluateResponse>, Status> {
        let req = request.into_inner();

        // Validate inputs
        if req.client_id.is_empty() {
            return Err(Status::invalid_argument("client_id is required"));
        }
        if req.client_secret.is_empty() {
            return Err(Status::invalid_argument("client_secret is required"));
        }
        if req.feature.is_empty() {
            return Err(Status::invalid_argument("feature (key) is required"));
        }

        let client_id = Uuid::parse_str(&req.client_id)
            .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;

        // Fetch client -> team
        let client_repo = client_repository(self.pool.clone());
        let client = client_repo
            .get_client_by_id(client_id)
            .await
            .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;

        // Validate client secret and status
        if !client.enabled {
            return Err(Status::permission_denied("client is disabled"));
        }
        if client.api_key != req.client_secret {
            return Err(Status::unauthenticated("invalid client_secret"));
        }

        let team_id = client.team_id;

        // Fetch feature by key within team
        let feature_repo = feature_repository(self.pool.clone());
        let mut features = feature_repo
            .get_features(team_id, Some(req.feature.clone()), None)
            .await
            .map_err(|e| Status::internal(format!("db error: {}", e)))?;

        let db_feature = features
            .pop()
            .ok_or_else(|| Status::not_found("feature with given key not found for client's team"))?;

        let eng_feature = self.map_db_feature_to_engine(db_feature.clone()).await?;

        let ec = engine::FeatureEvaluationContext {
            feature: db_feature.key,
            environment_id: req.environment_id,
            context: req
                .context
                .into_iter()
                .map(|c| engine::Context { key: c.key, value: c.value })
                .collect(),
        };

        let enabled = engine::evaluate(ec, eng_feature);

        Ok(Response::new(EvaluateResponse { enabled }))
    }
}

pub async fn serve(pool: sqlx::PgPool, addr: std::net::SocketAddr) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let svc = FeatureEvaluationSvc::new(pool);
    tonic::transport::Server::builder()
        .add_service(FeatureEvaluationServer::new(svc))
        .serve(addr)
        .await?;
    Ok(())
}
