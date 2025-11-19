# FluxGate Edge Server Performance Optimization Plan

**Created:** 2025-11-15
**Goal:** Reduce P95 latency from 18ms to 3-5ms and increase throughput from 100 RPS to 200-300 RPS

---

## 📊 Baseline Metrics

- **Current P95 Latency:** 18ms @ 100 RPS
- **Current Throughput:** 83 RPS (small/medium profiles)
- **Target P95 Latency:** 3-5ms
- **Target Throughput:** 200-300 RPS
- **Expected Overall Improvement:** 70-85% latency reduction, 2-3x throughput increase

---

## 🎯 Phase 1: Mapped Feature Cache (Priority: CRITICAL)

**Estimated Impact:** 40-60% latency reduction
**Effort:** Medium (2-3 days)
**Risk:** Low

### Task 1.1: Add MappedFeatureCache Struct
**File:** `src/main.rs`
**Location:** After `FeatureCache` struct (line 134)

**Implementation:**
```rust
/// Cache for pre-mapped engine::Feature to avoid repeated allocations
pub struct MappedFeatureCache {
    // Cache with Arc for zero-cost cloning
    cache: moka::future::Cache<String, Arc<engine::Feature>>,
}

impl MappedFeatureCache {
    pub fn new(max_capacity: u64) -> Self {
        tracing::info!("Initializing MappedFeatureCache with max_capacity={}", max_capacity);
        Self {
            cache: moka::future::Cache::new(max_capacity),
        }
    }

    pub async fn get(&self, key: &str) -> Option<Arc<engine::Feature>> {
        self.cache.get(key).await
    }

    pub async fn insert(&self, key: String, feature: Arc<engine::Feature>) {
        self.cache.insert(key, feature).await;
    }

    pub async fn invalidate(&self, key: &str) {
        self.cache.invalidate(key).await;
    }

    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }
}
```

**References:**
- See existing `FeatureCache` implementation: [main.rs:58-134](src/main.rs#L58-L134)
- See existing `ClientInfoCache` implementation: [main.rs:25-58](src/main.rs#L25-L58)

---

### Task 1.2: Add MappedFeatureCache to AppState
**File:** `src/main.rs`
**Location:** `AppState` struct (line 60-81)

**Changes:**
```rust
#[derive(Clone)]
pub struct AppState {
    cache: Arc<FeatureCache>,
    mapped_cache: Arc<MappedFeatureCache>,  // ADD THIS LINE
    client_info_cache: Arc<ClientInfoCache>,
    // ... rest of fields
}
```

**References:**
- Current `AppState` definition: [main.rs:60-81](src/main.rs#L60-L81)

---

### Task 1.3: Initialize MappedFeatureCache in main()
**File:** `src/main.rs`
**Location:** `main()` function where `AppState` is created (line 238-251)

**Changes:**
```rust
let state = AppState {
    cache: Arc::new(FeatureCache::new(cfg.cache.max_capacity)),
    mapped_cache: Arc::new(MappedFeatureCache::new(cfg.cache.max_capacity)),  // ADD THIS LINE
    client_info_cache: Arc::new(ClientInfoCache::new(cfg.cache.client_ttl())),
    // ... rest of initialization
};
```

**References:**
- Current initialization: [main.rs:238-251](src/main.rs#L238-L251)

---

### Task 1.4: Create Helper Function for Mapped Feature Retrieval
**File:** `src/handlers.rs`
**Location:** After `get_or_fetch_feature` function (line 191)

**Implementation:**
```rust
/// Get pre-mapped feature from cache or map and cache it
async fn get_or_map_feature(
    app: &AppState,
    feature_key: &str,
    client_id: &str,
    client_secret: &str,
) -> Option<Arc<engine::Feature>> {
    // Check mapped cache first
    if let Some(mapped) = app.mapped_cache.get(feature_key).await {
        return Some(mapped);
    }

    // Get protobuf feature from cache or backend
    let pb_feature = get_or_fetch_feature(app, feature_key, client_id, client_secret).await?;

    // Map to engine format
    let engine_feature = Arc::new(map_proto_to_engine(&pb_feature));

    // Cache the mapped version
    app.mapped_cache.insert(feature_key.to_string(), engine_feature.clone()).await;

    Some(engine_feature)
}
```

**References:**
- See `get_or_fetch_feature`: [handlers.rs:179-191](src/handlers.rs#L179-L191)
- See `map_proto_to_engine`: [handlers.rs:55-111](src/handlers.rs#L55-L111)

---

### Task 1.5: Update evaluate_handler to Use Mapped Cache
**File:** `src/handlers.rs`
**Location:** Lines 305-315 (both branches of the if-else)

**Current Code:**
```rust
// Lines 305-309
let engine_feature = map_proto_to_engine(&feature);
let ec = map_http_context_to_engine(feature_key.clone(), req.context.clone());
let result = engine::evaluate(ec, engine_feature);

// Lines 311-314
let engine_feature = map_proto_to_engine(&feature);
let ec = map_http_context_to_engine(feature_key.clone(), req.context.clone());
let result = engine::evaluate(ec, engine_feature);
```

**New Code:**
```rust
// Get pre-mapped feature ONCE before the if-else block (around line 288)
let mapped_feature = match get_or_map_feature(&app, &feature_key, &client_id, &client_secret).await {
    Some(f) => f,
    None => {
        return Ok(web::Json(EvaluateHttpResponse {
            flag_key: feature_key.clone(),
            value: serde_json::json!(false),
            variant: None,
            reason: "DEFAULT".to_string(),
            error_code: Some("FLAG_NOT_FOUND".to_string()),
            metadata: None,
        }));
    }
};

// Then in both branches (lines 305-309 and 311-314), replace with:
let ec = map_http_context_to_engine(feature_key.clone(), req.context.clone());
let result = engine::evaluate(ec, (*mapped_feature).clone());
```

**Note:** We need to clone the Arc's inner value only when calling evaluate since it takes ownership.

**References:**
- Current usage: [handlers.rs:305-315](src/handlers.rs#L305-L315)

---

### Task 1.6: Invalidate Mapped Cache on Feature Updates
**File:** `src/grpc_client.rs`
**Location:** `handle_feature_update` function (line 202-225)

**Changes:**
```rust
async fn handle_feature_update(app: &AppState, update: pb::FeatureUpdate) {
    use pb::feature_update::Action;
    match update.action {
        x if x == Action::Upsert as i32 || x == Action::Snapshot as i32 => {
            if let Some(f) = update.feature {
                let feature_id = f.id.clone();
                let feature_key = f.key.clone();  // ADD THIS LINE

                app.cache.upsert(f).await;
                app.mapped_cache.invalidate(&feature_key).await;  // ADD THIS LINE
                app.purge_assignments_for_feature(&feature_id).await;
            }
        }
        x if x == Action::Delete as i32 => {
            if !update.feature_key.is_empty() {
                app.mapped_cache.invalidate(&update.feature_key).await;  // ADD THIS LINE
                if let Some(feature_id) = app.cache.delete_by_key(&update.feature_key).await {
                    app.purge_assignments_for_feature(&feature_id).await;
                }
            }
        }
        _ => {}
    }
}
```

**References:**
- Current implementation: [grpc_client.rs:202-225](src/grpc_client.rs#L202-L225)

---

### Task 1.7: Update All Tests to Include MappedFeatureCache
**Files:** `src/main.rs`, `src/grpc_client.rs`
**Locations:** All test functions that create `AppState`

**Changes Required:**
1. `test_purge_assignments_for_feature` in [main.rs:326-390](src/main.rs#L326-L390)
2. `test_send_initial_subscribe_with_cached_keys` in [grpc_client.rs:498-576](src/grpc_client.rs#L498-L576)
3. `test_send_initial_subscribe_with_empty_cache` in [grpc_client.rs:578-615](src/grpc_client.rs#L578-L615)

**Add this line to each test:**
```rust
let mapped_cache = Arc::new(MappedFeatureCache::new(1000));

let app_state = crate::AppState {
    cache,
    mapped_cache,  // ADD THIS LINE
    client_info_cache,
    // ... rest
};
```

---

## 🎯 Phase 2: Lock-Free Data Structures (Priority: CRITICAL)

**Estimated Impact:** 20-30% latency reduction
**Effort:** Medium (2-3 days)
**Risk:** Low-Medium

### Task 2.1: Add DashMap Dependency
**File:** `Cargo.toml`
**Location:** `[dependencies]` section

**Add:**
```toml
dashmap = "6.1"
```

**References:**
- Current dependencies: [Cargo.toml](Cargo.toml)

---

### Task 2.2: Replace RwLock<HashMap> with DashMap for assigned_cache
**File:** `src/main.rs`
**Location:** `AppState` struct (line 73)

**Current:**
```rust
assigned_cache: Arc<RwLock<std::collections::HashMap<String, CachedAssignment>>>,
```

**New:**
```rust
assigned_cache: Arc<dashmap::DashMap<String, CachedAssignment>>,
```

**References:**
- Current definition: [main.rs:73](src/main.rs#L73)

---

### Task 2.3: Update assigned_cache Initialization
**File:** `src/main.rs`
**Location:** AppState initialization (line 245)

**Current:**
```rust
assigned_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
```

**New:**
```rust
assigned_cache: Arc::new(dashmap::DashMap::new()),
```

**References:**
- Current initialization: [main.rs:245](src/main.rs#L245)

---

### Task 2.4: Update assigned_cache Usage - Read Operations
**File:** `src/handlers.rs`
**Location:** Line 292

**Current:**
```rust
let cached = app.assigned_cache.read().await.get(&key).cloned();
```

**New:**
```rust
let cached = app.assigned_cache.get(&key).map(|entry| entry.value().clone());
```

**Note:** DashMap operations are synchronous and don't require .await

**References:**
- Current usage: [handlers.rs:292](src/handlers.rs#L292)

---

### Task 2.5: Update assigned_cache Usage - Write Operations
**File:** `src/handlers.rs`
**Location:** Lines 354-361

**Current:**
```rust
{
    let mut cache = app.assigned_cache.write().await;
    cache.insert(
        key,
        crate::CachedAssignment {
            value: result.value.clone(),
            variant: result.variant.clone(),
        },
    );
}
```

**New:**
```rust
app.assigned_cache.insert(
    key,
    crate::CachedAssignment {
        value: result.value.clone(),
        variant: result.variant.clone(),
    },
);
```

**References:**
- Current usage: [handlers.rs:354-361](src/handlers.rs#L354-L361)

---

### Task 2.6: Update purge_assignments_for_feature
**File:** `src/main.rs`
**Location:** `purge_assignments_for_feature` method (lines 137-154)

**Current:**
```rust
pub async fn purge_assignments_for_feature(&self, feature_id: &str) {
    {
        let mut cache = self.assigned_cache.write().await;
        let keys: Vec<String> = cache
            .keys()
            .filter(|entry| entry.split('|').nth(1) == Some(feature_id))
            .cloned()
            .collect();
        for key in keys {
            cache.remove(&key);
        }
    }
    // ... pending_assignments logic
}
```

**New:**
```rust
pub async fn purge_assignments_for_feature(&self, feature_id: &str) {
    // DashMap allows concurrent iteration and removal
    self.assigned_cache.retain(|key, _| {
        key.split('|').nth(1) != Some(feature_id)
    });

    // ... pending_assignments logic (no change needed)
}
```

**References:**
- Current implementation: [main.rs:137-154](src/main.rs#L137-L154)

---

### Task 2.7: Replace RwLock for pending_assignments with tokio::sync::Mutex
**File:** `src/main.rs`
**Location:** AppState struct (line 74)

**Current:**
```rust
pending_assignments: Arc<RwLock<Vec<grpc_client::UserAssignment>>>,
```

**New:**
```rust
pending_assignments: Arc<tokio::sync::Mutex<Vec<grpc_client::UserAssignment>>>,
```

**Rationale:** For write-heavy workloads, Mutex is faster than RwLock

**References:**
- Current definition: [main.rs:74](src/main.rs#L74)

---

### Task 2.8: Update pending_assignments Usage in handlers.rs
**File:** `src/handlers.rs`
**Location:** Line 363

**Current:**
```rust
let mut pending = app.pending_assignments.write().await;
```

**New:**
```rust
let mut pending = app.pending_assignments.lock().await;
```

**References:**
- Current usage: [handlers.rs:363](src/handlers.rs#L363)

---

### Task 2.9: Replace pending_evaluation_events with mpsc Channel
**File:** `src/main.rs`
**Location:** AppState struct (line 77)

**Current:**
```rust
pending_evaluation_events: Arc<RwLock<Vec<EvaluationEvent>>>,
```

**New:**
```rust
evaluation_event_tx: tokio::sync::mpsc::UnboundedSender<EvaluationEvent>,
evaluation_event_rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<EvaluationEvent>>>,
```

**Note:** This requires restructuring - we'll keep the sender in AppState and receiver in the flush task

**References:**
- Current definition: [main.rs:77](src/main.rs#L77)

---

### Task 2.10: Update AppState Initialization for Event Channel
**File:** `src/main.rs`
**Location:** Before AppState creation (line 238)

**Add:**
```rust
// Create unbounded channel for evaluation events
let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();

let state = AppState {
    // ... other fields
    evaluation_event_tx: event_tx,
    // Remove: pending_evaluation_events: Arc::new(RwLock::new(Vec::new())),
};

// Store receiver separately for flush task
let event_rx = Arc::new(tokio::sync::Mutex::new(event_rx));
```

**References:**
- Current initialization: [main.rs:238-251](src/main.rs#L238-L251)

---

### Task 2.11: Update Event Recording in evaluate_handler
**File:** `src/handlers.rs`
**Location:** Lines 341-344

**Current:**
```rust
{
    let mut pending_events = app.pending_evaluation_events.write().await;
    pending_events.push(evaluation_event);
}
```

**New:**
```rust
// Non-blocking send - if channel is full, this will panic (which is what we want for backpressure)
let _ = app.evaluation_event_tx.send(evaluation_event);
```

**References:**
- Current usage: [handlers.rs:341-344](src/handlers.rs#L341-L344)

---

### Task 2.12: Update run_evaluation_flush_task
**File:** `src/grpc_client.rs`
**Location:** `run_evaluation_flush_task` function (lines 381-483)

**Current:**
```rust
pub async fn run_evaluation_flush_task(app: AppState) {
    loop {
        tokio::time::sleep(app.evaluation_flush_interval).await;

        let to_send: Vec<crate::EvaluationEvent> = {
            let mut lock = app.pending_evaluation_events.write().await;
            if lock.is_empty() {
                Vec::new()
            } else {
                let v = lock.drain(..).collect::<Vec<_>>();
                v
            }
        };
        // ... rest
    }
}
```

**New:**
```rust
pub async fn run_evaluation_flush_task(
    app: AppState,
    mut event_rx: tokio::sync::mpsc::UnboundedReceiver<crate::EvaluationEvent>
) {
    let mut buffer = Vec::new();
    let flush_interval = app.evaluation_flush_interval;

    loop {
        tokio::time::sleep(flush_interval).await;

        // Drain all available events from channel
        while let Ok(event) = event_rx.try_recv() {
            buffer.push(event);
        }

        if buffer.is_empty() {
            continue;
        }

        let to_send = std::mem::take(&mut buffer);
        // ... rest of flush logic
    }
}
```

**References:**
- Current implementation: [grpc_client.rs:381-483](src/grpc_client.rs#L381-L483)

---

### Task 2.13: Update Flush Task Spawn
**File:** `src/main.rs`
**Location:** Where evaluation flush task is spawned (line 232-235)

**Current:**
```rust
let evaluation_flush_state = state.clone();
tokio::spawn(
    async move { grpc_client::run_evaluation_flush_task(evaluation_flush_state).await },
);
```

**New:**
```rust
let evaluation_flush_state = state.clone();
let event_rx_for_flush = Arc::try_unwrap(event_rx)
    .unwrap_or_else(|arc| panic!("Failed to unwrap event_rx"))
    .into_inner();
tokio::spawn(
    async move { grpc_client::run_evaluation_flush_task(evaluation_flush_state, event_rx_for_flush).await },
);
```

**References:**
- Current spawn: [main.rs:232-235](src/main.rs#L232-L235)

---

### Task 2.14: Update All Tests for New Lock Types
**Files:** `src/main.rs`, `src/grpc_client.rs`
**Locations:** All test functions

**Changes for assigned_cache:**
```rust
// OLD:
assigned_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),

// NEW:
assigned_cache: Arc::new(dashmap::DashMap::new()),
```

**Changes for pending_assignments:**
```rust
// OLD:
pending_assignments: Arc::new(RwLock::new(Vec::new())),

// NEW:
pending_assignments: Arc::new(tokio::sync::Mutex::new(Vec::new())),
```

**Changes for evaluation events:**
```rust
// OLD:
pending_evaluation_events: Arc::new(RwLock::new(Vec::new())),

// NEW: (in test setup)
let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
// Then in AppState:
evaluation_event_tx: event_tx,
```

---

## 🎯 Phase 3: Simple Feature Fast-Path (Priority: HIGH)

**Estimated Impact:** 10-15% latency reduction for 70% of features
**Effort:** Small (1 day)
**Risk:** Low

### Task 3.1: Add Simple Feature Detection
**File:** `src/handlers.rs`
**Location:** After getting the stage (around line 273)

**Implementation:**
```rust
let stage = stage.unwrap();

// Fast-path for simple features without criteria
if feature.feature_type == "Simple" && stage.criterias.is_empty() {
    // Simple feature with no criteria - just return enabled state
    let value = serde_json::json!(stage.enabled && feature.active);

    // Record simple evaluation event (without assignment tracking)
    let evaluation_event = EvaluationEvent {
        feature_key: feature.key.clone(),
        environment_id: req.context.environment_id.clone(),
        evaluation_result: value.as_bool().unwrap_or(false),
        evaluation_context: req.context.clone(),
        user_context: None,
        evaluated_at: std::time::SystemTime::now(),
        prior_assignment: false,
        variant: None,
    };
    let _ = app.evaluation_event_tx.send(evaluation_event);

    return Ok(web::Json(EvaluateHttpResponse {
        flag_key: feature_key,
        value,
        variant: None,
        reason: "STATIC".to_string(),
        error_code: None,
        metadata: None,
    }));
}

// Continue with normal evaluation for contextual features
```

**References:**
- Current code continues with full evaluation: [handlers.rs:273-389](src/handlers.rs#L273-L389)

---

## 🎯 Phase 4: Hash Caching (Priority: MEDIUM)

**Estimated Impact:** 5-10% latency reduction
**Effort:** Medium (2 days)
**Risk:** Low-Medium

### Task 4.1: Add BucketingCache Struct
**File:** `src/main.rs`
**Location:** After `MappedFeatureCache` implementation

**Implementation:**
```rust
/// Cache for pre-computed bucketing hash percentages
pub struct BucketingCache {
    // Cache (user_id, feature_key) -> bucket percentage
    cache: moka::future::Cache<(String, String), f32>,
}

impl BucketingCache {
    pub fn new(max_capacity: u64, ttl: Duration) -> Self {
        tracing::info!("Initializing BucketingCache with max_capacity={}, ttl={:?}", max_capacity, ttl);
        Self {
            cache: moka::future::Cache::builder()
                .max_capacity(max_capacity)
                .time_to_live(ttl)
                .build(),
        }
    }

    pub async fn get(&self, user_id: &str, feature_key: &str) -> Option<f32> {
        self.cache.get(&(user_id.to_string(), feature_key.to_string())).await
    }

    pub async fn insert(&self, user_id: String, feature_key: String, percentage: f32) {
        self.cache.insert((user_id, feature_key), percentage).await;
    }

    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }
}
```

---

### Task 4.2: Add Bucketing Cache Configuration
**File:** `src/config.rs`
**Location:** `CacheConfig` struct (line 97-105)

**Add:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub max_capacity: u64,
    pub client_ttl_secs: u64,

    /// Bucketing hash cache capacity (default: 100k entries)
    #[serde(default = "default_bucketing_capacity")]
    pub bucketing_capacity: u64,

    /// Bucketing hash cache TTL in seconds (default: 1 hour)
    #[serde(default = "default_bucketing_ttl")]
    pub bucketing_ttl_secs: u64,
}

fn default_bucketing_capacity() -> u64 {
    100_000
}

fn default_bucketing_ttl() -> u64 {
    3600 // 1 hour
}
```

---

### Task 4.3: Add BucketingCache to AppState
**File:** `src/main.rs`
**Location:** AppState struct

**Add:**
```rust
bucketing_cache: Arc<BucketingCache>,
```

---

### Task 4.4: Modify evaluation-engine to Support Hash Caching
**File:** `evaluation-engine/src/lib.rs`
**Location:** Add optional parameter to `passes_stage_criteria`

**Current signature (line 125):**
```rust
fn passes_stage_criteria(
    ec: &FeatureEvaluationContext,
    stage: &FeatureStage,
) -> CriteriaEvaluationResult
```

**New signature:**
```rust
fn passes_stage_criteria(
    ec: &FeatureEvaluationContext,
    stage: &FeatureStage,
    cached_bucket: Option<f32>,  // Pre-computed bucket percentage
) -> CriteriaEvaluationResult
```

**Update hash computation (lines 163-169):**
```rust
// Precompute user bucket percentage (or use cached value)
let user_bucket = if let Some(cached) = cached_bucket {
    cached
} else {
    let mut hasher = Sha256::new();
    hasher.update(ec.flag_key.as_bytes());
    hasher.update(b":");
    hasher.update(sticky_val.as_bytes());
    let digest = hasher.finalize();
    hash_to_percentage(&digest) // 0..100
};
```

---

### Task 4.5: Update evaluate() to Accept Cached Bucket
**File:** `evaluation-engine/src/lib.rs`
**Location:** `evaluate` function signature (line 215)

**Add parameter:**
```rust
pub fn evaluate(
    evaluation_context: FeatureEvaluationContext,
    feature: Feature,
    cached_bucket: Option<f32>,  // ADD THIS
) -> EvaluationResult
```

**Pass to `passes_stage_criteria` (line 280):**
```rust
let criteria_result = passes_stage_criteria(&evaluation_context, stage, cached_bucket);
```

---

### Task 4.6: Update Handler to Use Bucketing Cache
**File:** `src/handlers.rs`
**Location:** Before calling `engine::evaluate` (around line 305)

**Add:**
```rust
// Check bucketing cache
let cached_bucket = if let Some(user_id) = &user_id_opt {
    app.bucketing_cache.get(user_id, &feature_key).await
} else {
    None
};

// Call evaluate with cached bucket
let result = engine::evaluate(ec, (*mapped_feature).clone(), cached_bucket);

// Cache the bucket for future use (if not already cached and we computed it)
if cached_bucket.is_none() && user_id_opt.is_some() {
    // Extract bucket from evaluation result metadata if available
    // This requires modifying evaluation engine to return the computed bucket
}
```

---

## 🎯 Phase 5: Testing & Validation

### Task 5.1: Unit Tests for New Caches
**File:** `src/main.rs` (test module)

**Add tests:**
- `test_mapped_feature_cache_operations`
- `test_bucketing_cache_ttl`
- `test_dashmap_concurrent_access`

---

### Task 5.2: Integration Tests
**File:** Create `tests/performance_test.rs`

**Test scenarios:**
- Concurrent evaluation requests (100 RPS)
- Cache hit rates
- Lock contention metrics

---

### Task 5.3: Benchmarks
**File:** Create `benches/evaluation_bench.rs`

**Benchmark:**
- `map_proto_to_engine` vs cached mapped feature
- RwLock vs DashMap performance
- Hash computation vs cached bucket

---

### Task 5.4: Performance Test with k6
**After implementation, run:**
```bash
./run-perf-tests.sh --profiles "tiny small medium large" --quick
```

**Expected results:**
- P95 latency: < 5ms
- Throughput: > 200 RPS
- Error rate: < 0.1%

---

## 📋 Implementation Checklist

### Phase 1: Mapped Feature Cache
- [ ] Task 1.1: Add MappedFeatureCache struct
- [ ] Task 1.2: Add to AppState
- [ ] Task 1.3: Initialize in main()
- [ ] Task 1.4: Create get_or_map_feature helper
- [ ] Task 1.5: Update evaluate_handler
- [ ] Task 1.6: Invalidate on updates
- [ ] Task 1.7: Update tests

### Phase 2: Lock-Free Structures
- [ ] Task 2.1: Add DashMap dependency
- [ ] Task 2.2-2.6: Replace assigned_cache with DashMap
- [ ] Task 2.7-2.8: Update pending_assignments to Mutex
- [ ] Task 2.9-2.13: Replace evaluation events with channel
- [ ] Task 2.14: Update all tests

### Phase 3: Simple Feature Fast-Path
- [ ] Task 3.1: Add fast-path detection and early return

### Phase 4: Hash Caching
- [ ] Task 4.1: Add BucketingCache struct
- [ ] Task 4.2: Add configuration
- [ ] Task 4.3: Add to AppState
- [ ] Task 4.4-4.6: Integrate with evaluation engine

### Phase 5: Testing
- [ ] Task 5.1: Unit tests
- [ ] Task 5.2: Integration tests
- [ ] Task 5.3: Benchmarks
- [ ] Task 5.4: k6 performance tests

---

## 🔄 Rollback Plan

If any phase causes issues:

1. **Mapped Cache Issues:** Remove mapped_cache from AppState, revert to map_proto_to_engine
2. **DashMap Issues:** Revert to RwLock<HashMap>
3. **Channel Issues:** Revert to RwLock<Vec> for events
4. **Fast-Path Issues:** Remove early return logic

Each phase is independent and can be rolled back without affecting others.

---

## 📊 Success Metrics

Track these metrics before and after each phase:

- **P50 Latency:** Target < 2ms
- **P95 Latency:** Target < 5ms
- **P99 Latency:** Target < 10ms
- **Throughput:** Target > 200 RPS
- **CPU Usage:** Should decrease by 30-40%
- **Memory Usage:** May increase slightly due to caches
- **Cache Hit Rate:** > 95% for mapped features
- **Error Rate:** < 0.1%

---

## 📝 Notes

- All changes are backward compatible
- No API changes required
- Config changes are optional with sensible defaults
- Gradual rollout recommended (phase by phase)
- Monitor metrics after each phase before proceeding

---

**Ready to implement?** Start with Phase 1 (Mapped Feature Cache) for the biggest performance win!
