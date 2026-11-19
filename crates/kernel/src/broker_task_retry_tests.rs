//! Real PostgreSQL and loopback mTLS tests for explicitly retired Task Calls.
use super::*;
use crate::{broker_query, controller, error::Error};
use std::sync::atomic::AtomicUsize;

#[derive(Clone, Default)]
struct QueryProvider {
    invoke: Provider,
    query_count: Arc<AtomicUsize>,
    query_reported: Arc<AtomicUsize>,
    query_entered: Arc<Notify>,
    query_gate: Option<Arc<Semaphore>>,
}

#[tonic::async_trait]
impl rpc::capability_service_server::CapabilityService for QueryProvider {
    type InvokeStreamStream =
        <Provider as rpc::capability_service_server::CapabilityService>::InvokeStreamStream;

    async fn invoke(
        &self,
        request: tonic::Request<rpc::InvokeRequest>,
    ) -> Result<Response<rpc::InvokeResponse>, Status> {
        rpc::capability_service_server::CapabilityService::invoke(&self.invoke, request).await
    }

    async fn invoke_stream(
        &self,
        _: tonic::Request<rpc::InvokeRequest>,
    ) -> Result<Response<Self::InvokeStreamStream>, Status> {
        panic!("a unary Task must not call InvokeStream")
    }

    async fn cancel(
        &self,
        _: tonic::Request<rpc::CancelCallRequest>,
    ) -> Result<Response<rpc::Ack>, Status> {
        panic!("the retry fixture does not request cancellation")
    }

    async fn query_result(
        &self,
        request: tonic::Request<rpc::QueryResultRequest>,
    ) -> Result<Response<rpc::QueryResultResponse>, Status> {
        let query = tls::require_kernel(request)?.into_inner();
        let previous = self.invoke.requests.lock().await[0].clone();
        assert_eq!(query.call_id, previous.call_id);
        assert_eq!(
            query.idempotency_key,
            previous.context.unwrap().idempotency_key
        );
        assert!(!query.context.unwrap().delegation_token.is_empty());
        self.query_count.fetch_add(1, Ordering::SeqCst);
        self.query_entered.notify_one();
        if let Some(gate) = &self.query_gate {
            gate.acquire().await.unwrap().forget();
        }
        let output = br#"{"message":"verified by QueryResult"}"#.to_vec();
        self.query_reported.fetch_add(1, Ordering::SeqCst);
        Ok(Response::new(rpc::QueryResultResponse {
            state: rpc::ExecutionState::Succeeded as i32,
            outcome: Some(rpc::query_result_response::Outcome::Completed(
                rpc::InvokeResponse {
                    call_id: query.call_id,
                    output_digest: digest(&output),
                    output_json: output,
                    terminal_failure: None,
                },
            )),
        }))
    }
}

async fn submit(f: &Fixture, c: &Client, queryable: bool) -> Uuid {
    if queryable {
        let mut manifest: Value = c
            .query_one(
                "SELECT manifest FROM installations WHERE id=$1",
                &[&f.installation],
            )
            .await
            .unwrap()
            .get(0);
        manifest["capabilities"][0]["side_effect"] = json!("queryable");
        update_manifest(c, f.installation, &manifest).await;
        c.execute(
            "UPDATE capability_contracts SET spec=$1",
            &[&manifest["capabilities"][0]],
        )
        .await
        .unwrap();
    }
    let mut step = f.direct_step(f.deployment, 90_000);
    step["maximum_attempts"] = json!(2);
    f.submit_workflow("broker-explicit-retry", vec![step]).await
}

fn interrupted() -> Provider {
    Provider {
        reply: ProviderReply::Unavailable,
        ..Provider::default()
    }
}

async fn dispatch_unknown(f: &Fixture, c: &Client, task: Uuid) -> (AttemptLease, Uuid) {
    let lease = claim(f).await;
    let error = task_engine::dispatch(&f.app, &lease).await.unwrap_err();
    assert_eq!(error.code, "NOT_READY", "{error}");
    assert_eq!(task_state(c, task).await, ("unknown".into(), Value::Null));
    (lease, error.resource_id.unwrap().parse().unwrap())
}

async fn confirm(f: &Fixture, c: &Client, task: Uuid) {
    let row = c
        .query_one(
            "SELECT id,revision FROM task_steps WHERE task_id=$1",
            &[&task],
        )
        .await
        .unwrap();
    let step: Uuid = row.get(0);
    let resolved = f.post_with_revision(
        &format!("/api/platform/v1/task-steps/{step}/resolve"),
        "provider-journal-decision",
        json!({"resolution":"confirmed_not_executed", "evidence":"Fixture workload journal records no execution"}),
        Some(row.get(1)),
        200,
    ).await;
    assert_eq!(resolved["state"], "failed");
}

async fn requeue(f: &Fixture, c: &Client, task: Uuid, key: &str, status: u16) -> Value {
    let revision: i64 = c
        .query_one("SELECT revision FROM tasks WHERE id=$1", &[&task])
        .await
        .unwrap()
        .get(0);
    let mut request = Request::post(format!("/api/platform/v1/tasks/{task}/retry"))
        .header(
            "host",
            crate::plugin_http_gateway::core_authority(&f.app.config).unwrap(),
        )
        .header("authorization", format!("Bearer {}", f.token))
        .header("idempotency-key", key)
        .header("if-match", format!("\"rev-{revision}\""))
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50000".parse::<SocketAddr>().unwrap(),
    ));
    let response = f.router.clone().oneshot(request).await.unwrap();
    let actual = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(actual, status, "retryTask: {value}");
    value
}

async fn get(app: &App, token: &str, call: Uuid) -> Value {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    broker_query::get_call(
        app,
        &headers,
        "127.0.0.1:50000".parse().unwrap(),
        Uuid::new_v4(),
        call,
    )
    .await
    .unwrap()
}

async fn observe_restart(c: &Client, f: &Fixture) {
    c.execute("UPDATE instances SET phase='ready',health='alive',last_heartbeat_at=clock_timestamp() WHERE id=$1", &[&f.instance]).await.unwrap();
    c.execute("UPDATE provider_records SET availability='ready',observed_at=clock_timestamp() WHERE instance_id=$1", &[&f.instance]).await.unwrap();
}

async fn release_proof(c: &Client, call: Uuid) -> (String, i64) {
    let row = c.query_one(
        "SELECT r.status,(SELECT count(*) FROM audit_events a WHERE a.action='capability.invoke'
           AND a.reason_code='CALL_LEASE_EXPIRED' AND a.outcome='failed' AND a.credential_id IS NULL
           AND a.subject_id=c.owner_id AND a.resource->'ids'=jsonb_build_array(c.id))
          FROM calls c JOIN reservations r ON r.id=c.id WHERE c.id=$1", &[&call],
    ).await.unwrap();
    (row.get(0), row.get(1))
}

async fn old_work(c: &Client, task: Uuid) -> Value {
    c.query_one(
        "SELECT jsonb_agg(jsonb_build_array(to_jsonb(a),to_jsonb(c),to_jsonb(r)) ORDER BY a.number)
           FROM task_steps s JOIN attempts a ON a.step_id=s.id
           JOIN task_attempt_calls b ON b.attempt_id=a.id JOIN calls c ON c.id=b.call_id
           LEFT JOIN attempt_commit_receipts r ON r.attempt_id=a.id WHERE s.task_id=$1",
        &[&task],
    )
    .await
    .unwrap()
    .get(0)
}

#[tokio::test]
async fn broker_retry_demands_physical_stop_then_two_workers_send_one_successor() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.app.db.connect().await.unwrap();
    let task = submit(&f, &c, false).await;
    let previous = interrupted();
    let mut previous_server = Some(f.serve(f.instance, GENERATION, previous.clone()).await);
    let (old, call) = dispatch_unknown(&f, &c, task).await;
    confirm(&f, &c, task).await;
    requeue(&f, &c, task, "still-active", 409).await;
    assert_eq!(release_proof(&c, call).await, ("active".into(), 0));
    // A bulk ledger cleanup can release the slot without fencing this process.
    let set_slot = "UPDATE reservations SET status=$2 WHERE id=$1";
    c.execute(set_slot, &[&call, &"released"]).await.unwrap();
    requeue(&f, &c, task, "released-without-stop", 409).await;
    c.execute(set_slot, &[&call, &"active"]).await.unwrap();

    let instance_id = f.instance;
    controller::recover_expired_calls_with_stop(&f.app, |instance| async move {
        assert_eq!(instance, instance_id);
        Err(Error::unavailable("fixture physical stop failed"))
    })
    .await
    .unwrap_err();
    assert_eq!(release_proof(&c, call).await, ("active".into(), 0));
    requeue(&f, &c, task, "stop-failed", 409).await;
    controller::recover_expired_calls_with_stop(&f.app, |instance| {
        let mut server = previous_server.take().unwrap();
        async move {
            assert_eq!(instance, instance_id);
            server.0.abort();
            assert!((&mut server.0).await.unwrap_err().is_cancelled());
            Ok(())
        }
    })
    .await
    .unwrap();
    assert_eq!(release_proof(&c, call).await, ("released".into(), 1));
    let old_snapshot = old_work(&c, task).await;

    observe_restart(&c, &f).await;
    let successor = Provider::default();
    let _server = f.serve(f.instance, GENERATION, successor.clone()).await;
    let queued = requeue(&f, &c, task, "confirmed-physical-stop", 200).await;
    assert_eq!(queued["state"], "queued");
    assert_eq!(old_work(&c, task).await, old_snapshot);
    // Preexisting queued retry intents remain valid snapshot history even if
    // the old Controller did not produce a Call-specific stop audit.
    let legacy = c.transaction().await.unwrap();
    assert_eq!(
        legacy
            .execute(
                "DELETE FROM audit_events WHERE reason_code='CALL_LEASE_EXPIRED'
           AND resource->'ids'=jsonb_build_array($1::uuid)",
                &[&call],
            )
            .await
            .unwrap(),
        1
    );
    crate::tasks::snapshot::validate_snapshot(&legacy)
        .await
        .unwrap();
    legacy.rollback().await.unwrap();
    assert!(task_engine::claim(&f.app.db, &f.app.auth, Uuid::new_v4())
        .await
        .unwrap()
        .is_none());

    // A blocked reservation must not consume retry history or create an Attempt.
    let set_limit = "UPDATE quotas SET spec=jsonb_set(spec,'{maximum_concurrent_calls}',to_jsonb($2::integer)) WHERE account_id=$1";
    c.execute(set_limit, &[&f.owner, &0_i32]).await.unwrap();
    assert!(crate::broker::dispatch_next_task(&f.app, Uuid::new_v4())
        .await
        .unwrap()
        .is_none());
    assert_eq!(old_work(&c, task).await, old_snapshot);
    assert_eq!(task_state(&c, task).await.0, "queued");
    assert!(successor.requests.lock().await.is_empty());
    c.execute(set_limit, &[&f.owner, &16_i32]).await.unwrap();

    let set_digest = "UPDATE calls SET input_digest=$2 WHERE id=$1";
    c.execute(set_digest, &[&call, &digest(b"different input")])
        .await
        .unwrap();
    assert!(crate::broker::dispatch_next_task(&f.app, Uuid::new_v4())
        .await
        .unwrap()
        .is_none());
    assert_eq!(old_work(&c, task).await.as_array().unwrap().len(), 1);
    c.execute(
        set_digest,
        &[&call, &old_snapshot[0][1]["input_digest"].as_str().unwrap()],
    )
    .await
    .unwrap();

    let (a, b) = tokio::join!(
        crate::broker::dispatch_next_task(&f.app, Uuid::new_v4()),
        crate::broker::dispatch_next_task(&f.app, Uuid::new_v4()),
    );
    let applied = match (a.unwrap(), b.unwrap()) {
        (Some(step), None) | (None, Some(step)) => step,
        other => panic!("one retry must be dispatched: {other:?}"),
    };
    assert_eq!(applied["state"], "succeeded");
    let requests = successor.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let old_requests = previous.requests.lock().await;
    assert_eq!(old_requests.len(), 1);
    assert_ne!(requests[0].call_id, old_requests[0].call_id);
    assert_eq!(requests[0].input_json, old_requests[0].input_json);
    assert_eq!(requests[0].input_digest, old_requests[0].input_digest);
    assert_eq!(
        requests[0].context.as_ref().unwrap().idempotency_key,
        old_requests[0].context.as_ref().unwrap().idempotency_key
    );
    let history = old_work(&c, task).await;
    assert_eq!(history.as_array().unwrap().len(), 2);
    assert_eq!(history[0], old_snapshot[0]);
    assert_ne!(history[1][0]["id"], json!(old.attempt_id));
    assert_ne!(history[1][0]["execution_id"], json!(old.execution_id));
    assert_eq!(history[1][0]["fence"], old.fence + 1);
    assert_eq!(history[1][0]["number"], 2);
    crate::tasks::snapshot::validate_snapshot(&c).await.unwrap();
    assert_eq!(
        task_state(&c, task).await,
        ("succeeded".into(), json!({"message":"echo from workflow"}))
    );
    assert!(task_engine::dispatch(&f.app, &old).await.is_err());
}

#[tokio::test]
async fn broker_retry_refuses_a_terminal_query_result_that_committed_first() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    let task = submit(&f, &c, true).await;
    let provider = QueryProvider {
        invoke: interrupted(),
        ..QueryProvider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let (_, call) = dispatch_unknown(&f, &c, task).await;
    confirm(&f, &c, task).await;
    assert_eq!(get(&f.app, &f.token, call).await["state"], "succeeded");
    assert_eq!(provider.query_count.load(Ordering::SeqCst), 1);
    requeue(&f, &c, task, "query-won", 409).await;
    assert_eq!(task_state(&c, task).await.0, "failed");
    assert_eq!(old_work(&c, task).await.as_array().unwrap().len(), 1);
    assert_eq!(provider.invoke.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn broker_retry_barrier_rejects_a_late_query_after_the_query_lease_elapsed() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    let task = submit(&f, &c, true).await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = QueryProvider {
        invoke: interrupted(),
        query_gate: Some(gate.clone()),
        ..QueryProvider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let (_, call) = dispatch_unknown(&f, &c, task).await;
    confirm(&f, &c, task).await;
    let app = f.app.clone();
    let token = f.token.clone();
    let started = tokio::time::Instant::now();
    let pending = tokio::spawn(async move { get(&app, &token, call).await });
    tokio::time::timeout(Duration::from_secs(2), provider.query_entered.notified())
        .await
        .unwrap();

    // Simulate a stopped workload and READY observation of the same generation.
    // Keep the fixture transport open to let an already-sent reply reach getCall.
    let instance_id = f.instance;
    controller::recover_expired_calls_with_stop(&f.app, |instance| async move {
        assert_eq!(instance, instance_id);
        Ok(())
    })
    .await
    .unwrap();
    observe_restart(&c, &f).await;
    assert_eq!(release_proof(&c, call).await, ("released".into(), 1));
    requeue(&f, &c, task, "query-lease-live", 409).await;
    // Advance the durable expiry only, so the real response still arrives
    // before tonic times out and exercises post-wire reconciliation.
    assert_eq!(c.execute(
        "UPDATE rpc_delegations q SET expires_at=clock_timestamp()-interval '1 millisecond'
           FROM calls old WHERE q.call_id=old.id AND old.id=$1 AND q.execution_delegation_id<>old.delegation_id", &[&call],
    ).await.unwrap(), 1);
    let queued = requeue(&f, &c, task, "retry-won", 200).await;
    assert_eq!(queued["state"], "queued");
    let snapshot = old_work(&c, task).await;
    gate.add_permits(1);
    let retained = pending.await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(provider.query_reported.load(Ordering::SeqCst), 1);
    assert_eq!(retained["state"], "unknown");
    assert_eq!(retained["result"], Value::Null);
    assert_eq!(get(&f.app, &f.token, call).await, retained);
    assert_eq!(provider.query_count.load(Ordering::SeqCst), 1);
    assert_eq!(old_work(&c, task).await, snapshot);
}
