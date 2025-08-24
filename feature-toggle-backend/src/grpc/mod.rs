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
    feature_repo: Box<dyn crate::database::feature::FeatureRepository>,
    client_repo: Box<dyn crate::database::client::ClientRepository>,
    updates_tx: tokio::sync::broadcast::Sender<pb::FeatureUpdate>,
    // Tracks, per client_id, the set of feature keys that the client explicitly requested via GetFeatureByKeyRequest
    requested_keys: std::sync::Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<uuid::Uuid, std::collections::HashSet<String>>,
        >,
    >,
}

impl FeatureEvaluationSvc {
    pub fn new(
        pool: sqlx::PgPool,
        updates_tx: tokio::sync::broadcast::Sender<pb::FeatureUpdate>,
    ) -> Self {
        let feature_repo = crate::database::feature::feature_repository(pool.clone());
        let client_repo = crate::database::client::client_repository(pool.clone());
        Self {
            pool,
            feature_repo,
            client_repo,
            updates_tx,
            requested_keys: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    // test-friendly constructor to inject mocks
    pub fn new_with_repos(
        feature_repo: Box<dyn crate::database::feature::FeatureRepository>,
        client_repo: Box<dyn crate::database::client::ClientRepository>,
        updates_tx: tokio::sync::broadcast::Sender<pb::FeatureUpdate>,
    ) -> Self {
        // Use a dummy pool; not used when repos are injected
        let pool = sqlx::PgPool::connect_lazy("postgres://unused").expect("lazy pool");
        Self {
            pool,
            feature_repo,
            client_repo,
            updates_tx,
            requested_keys: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    async fn map_db_feature_to_engine(&self, f: db::Feature) -> Result<engine::Feature, Status> {
        let repo = &self.feature_repo;
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
        let repo = &self.feature_repo;

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
        let client_repo = &self.client_repo;
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
        let feature_repo = &self.feature_repo;
        let mut features = feature_repo
            .get_features(team_id, Some(req.feature_key.clone()), None)
            .await
            .map_err(|e| Status::internal(format!("db error: {}", e)))?;

        let db_feature = features.pop().ok_or_else(|| {
            Status::not_found("feature with given key not found for client's team")
        })?;

        let eng_feature = self.map_db_feature_to_engine(db_feature.clone()).await?;

        let ec = engine::FeatureEvaluationContext {
            feature: db_feature.key,
            environment_id: req.environment_id,
            context: req
                .context
                .into_iter()
                .map(|c| engine::Context {
                    key: c.key,
                    value: c.value,
                })
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
        let client_repo = &self.client_repo;
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
        let feature_repo = &self.feature_repo;
        let mut features = feature_repo
            .get_features(team_id, Some(req.feature_key.clone()), None)
            .await
            .map_err(|e| Status::internal(format!("db error: {}", e)))?;

        let db_feature = features.pop().ok_or_else(|| {
            Status::not_found("feature with given key not found for client's team")
        })?;

        let feature_msg = self.map_db_feature_to_full(db_feature).await?;

        // Track that this client requested this feature key for future update filtering
        {
            let mut map = self.requested_keys.write().await;
            let entry = map.entry(client_id).or_default();
            entry.insert(req.feature_key.clone());
        }

        Ok(Response::new(pb::GetFeatureByKeyResponse {
            feature: Some(feature_msg),
        }))
    }
    async fn push_user_assignments(
            &self,
            request: Request<tonic::Streaming<pb::UserFlagAssignment>>,
        ) -> Result<Response<pb::Ack>, Status> {
            let mut stream = request.into_inner();

            // Read first message to authenticate and then process the rest with same creds
            let first_msg = match stream.next().await {
                Some(Ok(m)) => m,
                Some(Err(e)) => return Err(Status::internal(format!("stream error: {}", e))),
                None => return Err(Status::invalid_argument("empty stream")),
            };
            if first_msg.client_id.is_empty() || first_msg.client_secret.is_empty() {
                return Err(Status::invalid_argument("client_id and client_secret are required"));
            }
            let client_id = Uuid::parse_str(&first_msg.client_id)
                .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;
            let client_repo = &self.client_repo;
            let client = client_repo
                .get_client_by_id(client_id)
                .await
                .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;
            if !client.enabled {
                return Err(Status::permission_denied("client is disabled"));
            }
            if client.api_key != first_msg.client_secret {
                return Err(Status::unauthenticated("invalid client_secret"));
            }

            // Helper to upsert a single assignment
            async fn upsert_assignment(
                pool: &sqlx::PgPool,
                user_id: &str,
                feature_id: &str,
                environment_id: &str,
                assigned: bool,
            ) -> Result<(), sqlx::Error> {
                // Convert to UUIDs
                let f_id = uuid::Uuid::parse_str(feature_id).map_err(|_| sqlx::Error::Decode("invalid feature_id".into()))?;
                let e_id = uuid::Uuid::parse_str(environment_id).map_err(|_| sqlx::Error::Decode("invalid environment_id".into()))?;
                sqlx::query(
                r#"INSERT INTO user_flag_assignments (user_id, feature_id, environment_id, assigned)
                   VALUES ($1, $2, $3, $4)
                   ON CONFLICT (user_id, feature_id, environment_id)
                   DO UPDATE SET assigned = EXCLUDED.assigned, assigned_at = now()"#
            )
            .bind(user_id)
            .bind(f_id)
            .bind(e_id)
            .bind(assigned)
            .execute(pool)
            .await?;
                Ok(())
            }

            // Process the first payload then the rest
            if !first_msg.user_id.is_empty() && !first_msg.feature_id.is_empty() && !first_msg.environment_id.is_empty() {
                upsert_assignment(&self.pool, &first_msg.user_id, &first_msg.feature_id, &first_msg.environment_id, first_msg.assigned)
                    .await
                    .map_err(|e| Status::internal(format!("db error: {}", e)))?;
            }

            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(m) => {
                        if !m.user_id.is_empty() && !m.feature_id.is_empty() && !m.environment_id.is_empty() {
                            if let Err(e) = upsert_assignment(&self.pool, &m.user_id, &m.feature_id, &m.environment_id, m.assigned).await {
                                return Err(Status::internal(format!("db error: {}", e)));
                            }
                        }
                    }
                    Err(e) => return Err(Status::internal(format!("stream error: {}", e))),
                }
            }

            Ok(Response::new(pb::Ack { message_id: uuid::Uuid::new_v4().to_string() }))
        }

        async fn list_user_assignments(
            &self,
            request: Request<pb::ListUserFlagAssignmentsRequest>,
        ) -> Result<Response<pb::ListUserFlagAssignmentsResponse>, Status> {
            let req = request.into_inner();
            if req.client_id.is_empty() || req.client_secret.is_empty() {
                return Err(Status::invalid_argument("client_id and client_secret are required"));
            }
            let client_id = Uuid::parse_str(&req.client_id)
                .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;
            let client_repo = &self.client_repo;
            let client = client_repo
                .get_client_by_id(client_id)
                .await
                .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;
            if !client.enabled {
                return Err(Status::permission_denied("client is disabled"));
            }
            if client.api_key != req.client_secret {
                return Err(Status::unauthenticated("invalid client_secret"));
            }
            let team_id = client.team_id;

            // Build and execute query joining features to enforce team scoping
            #[derive(sqlx::FromRow)]
            struct AssignmentRow {
                user_id: String,
                feature_id: uuid::Uuid,
                environment_id: uuid::Uuid,
                assigned: bool,
            }
            let rows: Vec<AssignmentRow> = if !req.feature_id.is_empty() && !req.environment_id.is_empty() {
                let fid = uuid::Uuid::parse_str(&req.feature_id)
                    .map_err(|_| Status::invalid_argument("feature_id must be a UUID"))?;
                let eid = uuid::Uuid::parse_str(&req.environment_id)
                    .map_err(|_| Status::invalid_argument("environment_id must be a UUID"))?;
                sqlx::query_as!(AssignmentRow,
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned
                       FROM user_flag_assignments ufa
                       JOIN features f ON f.id = ufa.feature_id
                       WHERE f.team_id = $1 AND ufa.feature_id = $2 AND ufa.environment_id = $3"#,
                    team_id,
                    fid,
                    eid
                )
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?
            } else if !req.feature_id.is_empty() {
                let fid = uuid::Uuid::parse_str(&req.feature_id)
                    .map_err(|_| Status::invalid_argument("feature_id must be a UUID"))?;
                sqlx::query_as!(AssignmentRow,
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned
                       FROM user_flag_assignments ufa
                       JOIN features f ON f.id = ufa.feature_id
                       WHERE f.team_id = $1 AND ufa.feature_id = $2"#,
                    team_id,
                    fid
                )
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?
            } else if !req.environment_id.is_empty() {
                let eid = uuid::Uuid::parse_str(&req.environment_id)
                    .map_err(|_| Status::invalid_argument("environment_id must be a UUID"))?;
                sqlx::query_as!(AssignmentRow,
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned
                       FROM user_flag_assignments ufa
                       JOIN features f ON f.id = ufa.feature_id
                       WHERE f.team_id = $1 AND EXISTS (
                           SELECT 1 FROM features_pipeline_stages s
                           WHERE s.feature_id = f.id AND s.environment_id = $2
                       )"#,
                    team_id,
                    eid
                )
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?
            } else {
                sqlx::query_as!(AssignmentRow,
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned
                       FROM user_flag_assignments ufa
                       JOIN features f ON f.id = ufa.feature_id
                       WHERE f.team_id = $1"#,
                    team_id
                )
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?
            };

            let assignments = rows
                .into_iter()
                .map(|r| pb::UserFlagAssignment {
                    user_id: r.user_id,
                    feature_id: r.feature_id.to_string(),
                    environment_id: r.environment_id.to_string(),
                    assigned: r.assigned,
                    client_id: String::new(),
                    client_secret: String::new(),
                })
                .collect::<Vec<_>>();

            Ok(Response::new(pb::ListUserFlagAssignmentsResponse { assignments }))
        }

        type StreamUpdatesStream = ReceiverStream<Result<pb::FeatureUpdate, Status>>;

    async fn stream_updates(
        &self,
        request: Request<tonic::Streaming<pb::StreamRequest>>,
    ) -> Result<Response<Self::StreamUpdatesStream>, Status> {
        let mut in_stream = request.into_inner();

        // Expect first message to be SubscribeRequest
        let first = in_stream
            .next()
            .await
            .ok_or_else(|| Status::invalid_argument("missing subscribe message"))??;
        let subscribe = match first.payload {
            Some(pb::stream_request::Payload::Subscribe(s)) => s,
            _ => return Err(Status::invalid_argument("first message must be subscribe")),
        };

        // Authenticate similar to other methods
        if subscribe.client_id.is_empty() || subscribe.client_secret.is_empty() {
            return Err(Status::invalid_argument(
                "client_id and client_secret are required",
            ));
        }
        let client_id = Uuid::parse_str(&subscribe.client_id)
            .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;
        let client_repo = &self.client_repo;
        let client = client_repo
            .get_client_by_id(client_id)
            .await
            .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;
        if !client.enabled {
            return Err(Status::permission_denied("client is disabled"));
        }
        if client.api_key != subscribe.client_secret {
            return Err(Status::unauthenticated("invalid client_secret"));
        }
        let team_id = client.team_id;

        // Prepare outgoing channel
        let (out_tx, out_rx) = mpsc::channel::<Result<pb::FeatureUpdate, Status>>(64);

        // Send initial snapshot: only for features that were previously requested via GetFeatureByKeyRequest for this client.
        {
            let feature_repo = &self.feature_repo;
            let keys_snapshot: Vec<String> = {
                let map = self.requested_keys.read().await;
                map.get(&client_id)
                    .map(|set| set.iter().cloned().collect())
                    .unwrap_or_else(Vec::new)
            };

            for k in keys_snapshot {
                let mut features = feature_repo
                    .get_features(team_id, Some(k.clone()), None)
                    .await
                    .map_err(|e| Status::internal(format!("db error: {}", e)))?;
                if let Some(f) = features.pop() {
                    let full = self.map_db_feature_to_full(f).await?;
                    let _ = out_tx
                        .send(Ok(pb::FeatureUpdate {
                            message_id: uuid::Uuid::new_v4().to_string(),
                            action: pb::feature_update::Action::Snapshot as i32,
                            feature: Some(full),
                            feature_key: String::new(),
                            error: String::new(),
                        }))
                        .await;
                }
            }
        }

        // Subscribe to shared broadcaster for live updates, filtering per client's requested keys
        let mut rx = self.updates_tx.subscribe();
        let out_tx_clone = out_tx.clone();
        let requested_keys = self.requested_keys.clone();
        let client_id_for_filter = client_id;
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(update) => {
                        // Determine feature key for the update
                        let key_for_update = if let Some(ref feature) = update.feature {
                            feature.key.clone()
                        } else {
                            update.feature_key.clone()
                        };
                        // Check if the client has requested this key (dynamic check)
                        let should_send = {
                            let map = requested_keys.read().await;
                            map.get(&client_id_for_filter)
                                .map(|set| set.contains(&key_for_update))
                                .unwrap_or(false)
                        };
                        if should_send && out_tx_clone.send(Ok(update)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let _ = out_tx_clone
                            .send(Ok(pb::FeatureUpdate {
                                message_id: uuid::Uuid::new_v4().to_string(),
                                action: pb::feature_update::Action::Error as i32,
                                feature: None,
                                feature_key: String::new(),
                                error: "lagged".into(),
                            }))
                            .await;
                    }
                }
            }
        });

        // Handle incoming heartbeats/acks (optional). We keep the stream alive by draining inputs.
        let drain_tx = out_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = in_stream.next().await {
                match msg {
                    Ok(pb::StreamRequest {
                        payload: Some(pb::stream_request::Payload::Heartbeat(hb)),
                    }) => {
                        let _ = drain_tx
                            .send(Ok(pb::FeatureUpdate {
                                message_id: uuid::Uuid::new_v4().to_string(),
                                action: pb::feature_update::Action::Heartbeat as i32,
                                feature: None,
                                feature_key: String::new(),
                                error: String::new(),
                            }))
                            .await;
                    }
                    Ok(_) => { /* ignore other kinds for now */ }
                    Err(_) => break,
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(out_rx)))
    }
}

pub async fn serve(
    pool: sqlx::PgPool,
    addr: std::net::SocketAddr,
    updates_tx: broadcast::Sender<pb::FeatureUpdate>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let svc = FeatureEvaluationSvc::new(pool, updates_tx.clone());
    tonic::transport::Server::builder()
        .add_service(FeatureEvaluationServer::new(svc))
        .serve(addr)
        .await?;
    Ok(())
}
