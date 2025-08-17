pub mod pb {
    tonic::include_proto!("featuretoggle");
}

use crate::database::client::client_repository;
use crate::database::entity as db;
use crate::database::feature::feature_repository;
use evaluation_engine as engine;
use futures_util::StreamExt;
use pb::feature_evaluation_server::{FeatureEvaluation, FeatureEvaluationServer};
use pb::{EvaluateRequest, EvaluateResponse};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use uuid::Uuid;

pub use pb::feature_evaluation_server;
// re-export for server creation

#[derive(Clone)]
pub struct FeatureEvaluationSvc {
    pool: sqlx::PgPool,
    updates_tx: tokio::sync::broadcast::Sender<pb::FeatureUpdate>,
}

impl FeatureEvaluationSvc {
    pub fn new(pool: sqlx::PgPool, updates_tx: tokio::sync::broadcast::Sender<pb::FeatureUpdate>) -> Self {
        Self { pool, updates_tx }
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

    async fn map_db_feature_to_full(&self, f: db::Feature) -> Result<pb::FeatureFull, Status> {
        let repo = feature_repository(self.pool.clone());

        // Map stages and load criterias for each
        let mut stage_msgs: Vec<pb::FeatureStageFull> = Vec::with_capacity(f.stages.len());
        for s in f.stages.iter() {
            let crits = repo
                .get_stage_criteria(s.id)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?;
            let criterias = crits
                .into_iter()
                .map(|c| pb::StageCriterionFull {
                    id: c.id.to_string(),
                    context_key: c.context_key,
                    context: Some(pb::CriterionContext {
                        key: c.context.key,
                        entries: c.context.entries.into_iter().map(|e| e.value).collect(),
                    }),
                    rollout_percentage: c.rollout_percentage,
                })
                .collect::<Vec<_>>();

            stage_msgs.push(pb::FeatureStageFull {
                id: s.id.to_string(),
                environment_id: s.environment_id.to_string(),
                order_index: s.order_index,
                position: s.position.clone(),
                enabled: s.enabled,
                bucketing_key: s.bucketing_key.clone().unwrap_or_default(),
                criterias,
            });
        }

        // Map dependencies
        let deps = f
            .dependencies
            .iter()
            .map(|d| pb::FeatureDependencyFull {
                feature_id: d.feature_id.to_string(),
                depends_on_id: d.depends_on_id.to_string(),
            })
            .collect::<Vec<_>>();

        let feature = pb::FeatureFull {
            id: f.id.to_string(),
            key: f.key,
            description: f.description.unwrap_or_default(),
            feature_type: format!("{:?}", f.feature_type),
            team_id: f.team_id.to_string(),
            created_at: f.created_at.to_rfc3339(),
            stages: stage_msgs,
            dependencies: deps,
        };

        Ok(feature)
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
        if req.feature_key.is_empty() {
            return Err(Status::invalid_argument("feature_key is required"));
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
            .get_features(team_id, Some(req.feature_key.clone()), None)
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

    async fn get_feature_by_key(
        &self,
        request: Request<pb::GetFeatureByKeyRequest>,
    ) -> Result<Response<pb::GetFeatureByKeyResponse>, Status> {
        let req = request.into_inner();

        if req.client_id.is_empty() {
            return Err(Status::invalid_argument("client_id is required"));
        }
        if req.client_secret.is_empty() {
            return Err(Status::invalid_argument("client_secret is required"));
        }
        if req.feature_key.is_empty() {
            return Err(Status::invalid_argument("feature_key is required"));
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
            .get_features(team_id, Some(req.feature_key.clone()), None)
            .await
            .map_err(|e| Status::internal(format!("db error: {}", e)))?;

        let db_feature = features
            .pop()
            .ok_or_else(|| Status::not_found("feature with given key not found for client's team"))?;

        let feature_msg = self.map_db_feature_to_full(db_feature).await?;

        Ok(Response::new(pb::GetFeatureByKeyResponse { feature: Some(feature_msg) }))
    }
    type StreamUpdatesStream = ReceiverStream<Result<pb::FeatureUpdate, Status>>;

    async fn stream_updates(
        &self,
        request: Request<tonic::Streaming<pb::StreamRequest>>,
    ) -> Result<Response<Self::StreamUpdatesStream>, Status> {
        let mut in_stream = request.into_inner();

        // Expect first message to be SubscribeRequest
        let first = in_stream.next().await.ok_or_else(|| Status::invalid_argument("missing subscribe message"))??;
        let subscribe = match first.payload {
            Some(pb::stream_request::Payload::Subscribe(s)) => s,
            _ => return Err(Status::invalid_argument("first message must be subscribe")),
        };

        // Authenticate similar to other methods
        if subscribe.client_id.is_empty() || subscribe.client_secret.is_empty() {
            return Err(Status::invalid_argument("client_id and client_secret are required"));
        }
        let client_id = Uuid::parse_str(&subscribe.client_id)
            .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;
        let client_repo = client_repository(self.pool.clone());
        let client = client_repo
            .get_client_by_id(client_id)
            .await
            .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;
        if !client.enabled { return Err(Status::permission_denied("client is disabled")); }
        if client.api_key != subscribe.client_secret { return Err(Status::unauthenticated("invalid client_secret")); }
        let team_id = client.team_id;

        // Prepare outgoing channel
        let (out_tx, out_rx) = mpsc::channel::<Result<pb::FeatureUpdate, Status>>(64);

        // Send initial snapshot (filtered by keys if provided)
        {
            let feature_repo = feature_repository(self.pool.clone());
            let keys = if subscribe.feature_keys.is_empty() { None } else { Some(subscribe.feature_keys.join(",")) };
            // If keys provided, we'll fetch one-by-one to reuse existing repository API
            if let Some(list) = keys {
                for k in list.split(',') {
                    let mut features = feature_repo
                        .get_features(team_id, Some(k.to_string()), None)
                        .await
                        .map_err(|e| Status::internal(format!("db error: {}", e)))?;
                    if let Some(f) = features.pop() {
                        let full = self.map_db_feature_to_full(f).await?;
                        let _ = out_tx.send(Ok(pb::FeatureUpdate { message_id: uuid::Uuid::new_v4().to_string(), action: pb::feature_update::Action::Snapshot as i32, feature: Some(full), feature_key: String::new(), error: String::new() })).await;
                    }
                }
            } else {
                // Fetch all features for team (no key filter)
                let features = feature_repo
                    .get_features(team_id, None, None)
                    .await
                    .map_err(|e| Status::internal(format!("db error: {}", e)))?;
                for f in features {
                    let full = self.map_db_feature_to_full(f).await?;
                    let _ = out_tx.send(Ok(pb::FeatureUpdate { message_id: uuid::Uuid::new_v4().to_string(), action: pb::feature_update::Action::Snapshot as i32, feature: Some(full), feature_key: String::new(), error: String::new() })).await;
                }
            }
        }

        // Subscribe to shared broadcaster for live updates
        let mut rx = self.updates_tx.subscribe();
        let out_tx_clone = out_tx.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(update) => {
                        if out_tx_clone.send(Ok(update)).await.is_err() { break; }
                    },
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let _ = out_tx_clone.send(Ok(pb::FeatureUpdate { message_id: uuid::Uuid::new_v4().to_string(), action: pb::feature_update::Action::Error as i32, feature: None, feature_key: String::new(), error: "lagged".into() })).await;
                    }
                }
            }
        });

        // Handle incoming heartbeats/acks (optional). We keep the stream alive by draining inputs.
        let drain_tx = out_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = in_stream.next().await {
                match msg {
                    Ok(pb::StreamRequest { payload: Some(pb::stream_request::Payload::Heartbeat(hb)) }) => {
                        let _ = drain_tx.send(Ok(pb::FeatureUpdate { message_id: uuid::Uuid::new_v4().to_string(), action: pb::feature_update::Action::Heartbeat as i32, feature: None, feature_key: String::new(), error: String::new() })).await;
                    }
                    Ok(_) => { /* ignore other kinds for now */ }
                    Err(_) => break,
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(out_rx)))
    }
}

pub async fn serve(pool: sqlx::PgPool, addr: std::net::SocketAddr, updates_tx: broadcast::Sender<pb::FeatureUpdate>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let svc = FeatureEvaluationSvc::new(pool, updates_tx.clone());
    tonic::transport::Server::builder()
        .add_service(FeatureEvaluationServer::new(svc))
        .serve(addr)
        .await?;
    Ok(())
}
