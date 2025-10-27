# Edge Server Retry - Example Scenarios

## Scenario 1: Brief Network Hiccup During Feature Fetch

### Timeline
```
T+0ms:    Client requests feature evaluation
T+1ms:    Edge calls fetch_feature_via_grpc
T+2ms:    First attempt fails (network timeout)
T+502ms:  Second attempt (after 500ms backoff)
T+503ms:  Success! Feature fetched
T+504ms:  Client receives response
```

### Logs
```
ERROR gRPC GetFeatureByKey error after retries for feature 'BetaFeature': ...
INFO Successfully fetched feature: BetaFeature
```

### Result
✅ Request succeeds transparently, user unaware of underlying issue

---

## Scenario 2: Backend Server Restart

### Timeline
```
T+0s:     Edge server streaming from backend
T+1s:     Backend crashes/restarts
T+1.1s:   Stream connection breaks
T+1.1s:   ERROR: "stream recv error: ..."
T+1.1s:   WARN: "Stream connection lost, will reconnect in 1s"
T+2.1s:   Reconnection attempt 1 fails
T+2.1s:   ERROR: "Failed to connect to backend gRPC..."
T+2.1s:   WARN: "Retrying connection in 1s"
T+4.1s:   Reconnection attempt 2 fails  
T+4.1s:   Delay doubled to 2s
T+8.1s:   Reconnection attempt 3 fails
T+8.1s:   Delay doubled to 4s
T+16.1s:  Backend back online
T+16.1s:  Reconnection succeeds!
T+16.1s:  INFO: "Connected to backend gRPC http://backend:50051"
T+16.2s:  INFO: "Stream connection established, receiving updates"
T+16.2s:  Delay reset to 1s for next reconnection (if needed)
```

### During Outage
- Direct requests: Retry 3 times then fail with 502
- Cached features: Continue to work (if cached)
- Health endpoint: Returns 503

### Result
✅ Automatic recovery, no manual intervention needed

---

## Scenario 3: Evaluation Events Push Failure

### Timeline
```
T+0s:     30 seconds elapsed, time to flush evaluation events
T+0s:     100 events collected
T+0.1s:   Convert events to proto format
T+0.2s:   First push attempt fails (backend overloaded)
T+0.7s:   Second attempt (after 500ms backoff)
T+0.8s:   Still fails
T+1.7s:   Third attempt (after 1s backoff)
T+1.8s:   Still fails
T+3.7s:   Fourth attempt (after 2s backoff)
T+3.8s:   Success!
```

### Logs
```
INFO Successfully pushed 100 evaluation events
```

### Result
✅ All events delivered, no data loss

---

## Scenario 4: Prolonged Backend Outage

### Timeline
```
T+0s:     Backend goes offline (maintenance)
T+0s:     Stream disconnects
T+1s:     Retry with 1s delay → Fail
T+2s:     Retry with 1s delay → Fail
T+4s:     Retry with 2s delay → Fail
T+8s:     Retry with 4s delay → Fail
T+16s:    Retry with 8s delay → Fail
T+32s:    Retry with 16s delay → Fail
T+62s:    Retry with 30s delay → Fail (max delay reached)
T+92s:    Retry with 30s delay → Fail
T+122s:   Retry with 30s delay → Fail
...       (continues with 30s delays)
T+600s:   Backend comes back online
T+600.1s: Retry succeeds!
T+600.1s: Stream reconnects, delay resets to 1s
```

### During Outage (10 minutes)
- **Direct requests**: All fail after 3 retries (3.5s total delay per request)
- **Cached features**: Continue to evaluate correctly
- **Health endpoint**: Returns 503
- **User assignments**: Queue up, will flush when connection restored
- **Evaluation events**: Queue up, will flush when connection restored

### Result
⚠️ Some degraded functionality, but:
- No thundering herd when backend recovers
- Cached features still work
- Data queued for delivery
- Automatic recovery

---

## Scenario 5: Intermittent Connection Issues

### Timeline
```
T+0s:     Stream connected, everything working
T+5s:     Brief network blip
T+5.1s:   Stream breaks
T+6.1s:   Reconnect succeeds (1s delay)
T+6.2s:   Delay reset to 1s
...
T+20s:    Another blip
T+20.1s:  Stream breaks  
T+21.1s:  Reconnect succeeds (1s delay)
T+21.2s:  Delay reset to 1s
...
T+50s:    Prolonged issue starts
T+50s:    Stream breaks
T+51s:    Retry → Fail (1s delay)
T+53s:    Retry → Fail (2s delay)
T+57s:    Retry → Fail (4s delay)
T+65s:    Retry → Success (8s delay)
T+65.1s:  Delay reset to 1s
```

### Adaptive Behavior
- Quick recovery from brief issues (1s delay)
- Slower retry rate during persistent problems
- Always resets to fast retry after successful connection

### Result
✅ Optimal balance between fast recovery and avoiding overload

---

## Scenario 6: Client Fetching Feature with Retry

### Request Flow
```
1. Client → Edge: POST /evaluate {"feature_key": "NewUI", ...}
2. Edge checks cache → Not found
3. Edge → fetch_client_info_via_grpc("client123")
   Attempt 1 → Timeout (500ms)
   Attempt 2 → Success (after 500ms backoff)
   → Got client info
4. Edge → fetch_feature_via_grpc("NewUI")  
   Attempt 1 → Connection refused
   Attempt 2 → Connection refused (after 500ms)
   Attempt 3 → Success (after 1s backoff)
   → Got feature definition
5. Edge evaluates feature with engine
6. Edge → Client: 200 OK {"enabled": true}

Total time: ~2 seconds (including retries)
```

### Logs
```
INFO Successfully fetched client info for: client123
INFO Successfully fetched feature: NewUI
```

### Result
✅ Request succeeds despite 4 total failures (2 in each call)

---

## Comparison: Before vs After

### Before Implementation

**Network Blip Scenario:**
```
T+0s:   Request arrives
T+0.1s: Call backend → Timeout
T+0.1s: Return 502 to client ❌
```
- User sees error immediately
- No automatic recovery
- Operator may need to investigate

**Backend Restart:**
```
T+0s:   Stream disconnects
T+3s:   Retry
T+6s:   Retry
T+9s:   Retry
...
```
- Fixed 3s delay even for quick restarts
- Slow recovery (9+ seconds)

### After Implementation

**Network Blip Scenario:**
```
T+0s:     Request arrives
T+0.1s:   Call backend → Timeout
T+0.6s:   Retry → Success
T+0.6s:   Return 200 to client ✅
```
- User gets correct response
- Transparent to client
- No operator intervention

**Backend Restart:**
```
T+0s:   Stream disconnects
T+1s:   Retry → Success (if backend quick)
OR
T+1s:   Retry → Fail
T+2s:   Retry → Fail  
T+4s:   Retry → Success (if backend slower)
```
- Fast recovery for quick restarts (1s)
- Exponential backoff for longer outages
- Automatic, no manual steps

---

## Key Takeaways

1. **Transient errors are hidden from users** - Most network blips resolve within the retry window
2. **Exponential backoff prevents overload** - System doesn't hammer backend during outages
3. **Fast recovery from brief issues** - 1-second initial retry catches most restarts
4. **Automatic recovery** - No manual intervention needed in most cases
5. **Intelligent queueing** - Failed operations queue up and retry, minimizing data loss
