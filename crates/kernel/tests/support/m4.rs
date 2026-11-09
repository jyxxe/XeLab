//! Actual signed OCI consumer -> workload mTLS HostService -> fixed provider,
//! Skill and durable event paths. No SQL writes manufacture routing/health.
use super::*;

struct ConsumerScope<'a> {
    app: &'a Arc<App>,
    router: &'a Router,
    token: &'a str,
    provider: Uuid,
    provider_installation: Uuid,
    provider_instance: Uuid,
    skill: Uuid,
    subscription: Uuid,
    topic: &'a str,
    invocation_contract: &'a Value,
    dependency_contract: &'a Value,
    instances: &'a [Uuid],
}

struct ConsumerReceipt {
    call: Uuid,
    children: [Uuid; 2],
    event: Uuid,
}

// Never print an arbitrary workload error message. This parser deliberately
// duplicates the fixture's closed vocabulary rather than trusting its JSON.
fn safe_diagnostic(raw: &str) -> Option<Value> {
    const STAGES: &[&str] = &[
        "host.open",
        "dependency.resolve",
        "dependency.resolve.validate",
        "dependency.invoke",
        "dependency.invoke.validate",
        "skill.resolve",
        "skill.resolve.validate",
        "skill.check",
        "skill.check.validate",
        "skill.invoke",
        "skill.invoke.validate",
        "event.publish",
        "event.publish.duplicate",
        "event.publish.validate",
        "event.subscribe",
        "event.delivery.validate",
        "event.ack",
        "event.ack.duplicate",
        "event.ack.validate",
        "unknown",
    ];
    const KINDS: &[&str] = &[
        "RPC",
        "ValueError",
        "KeyError",
        "TypeError",
        "StopIteration",
        "OSError",
    ];
    const STATUSES: &[&str] = &[
        "OK",
        "CANCELLED",
        "UNKNOWN",
        "INVALID_ARGUMENT",
        "DEADLINE_EXCEEDED",
        "NOT_FOUND",
        "ALREADY_EXISTS",
        "PERMISSION_DENIED",
        "RESOURCE_EXHAUSTED",
        "FAILED_PRECONDITION",
        "ABORTED",
        "OUT_OF_RANGE",
        "UNIMPLEMENTED",
        "INTERNAL",
        "UNAVAILABLE",
        "DATA_LOSS",
        "UNAUTHENTICATED",
    ];
    const CHECKS: &[&str] = &[
        "binding_contract_equal",
        "binding_deadline_within_parent",
        "binding_unexpired",
        "skill_ready",
        "skill_target_equal",
        "skill_access_ready",
        "event_duplicate_same",
        "delivery_event_equal",
    ];
    if raw.len() > 1024 {
        return None;
    }
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    if object.get("version")?.as_u64()? != 1
        || object.get("code")?.as_str()? != "M4_FIXTURE_DIAGNOSTIC"
    {
        return None;
    }
    for (field, allowed) in [("stage", STAGES), ("kind", KINDS), ("status", STATUSES)] {
        let label = object.get(field)?.as_str()?;
        if label.len() > 64 || !label.is_ascii() || !allowed.contains(&label) {
            return None;
        }
    }
    for (name, value) in object {
        if !["version", "code", "stage", "kind", "status"].contains(&name.as_str())
            && (!CHECKS.contains(&name.as_str()) || !value.is_boolean())
        {
            return None;
        }
    }
    Some(value)
}

#[test]
fn m4_diagnostic_metadata_rejects_untrusted_fields_and_labels() {
    let valid = json!({"version":1,"code":"M4_FIXTURE_DIAGNOSTIC","stage":"dependency.resolve.validate","kind":"ValueError","status":"FAILED_PRECONDITION","binding_contract_equal":false});
    assert_eq!(safe_diagnostic(&valid.to_string()), Some(valid.clone()));
    for (field, value) in [
        ("version", json!(2)),
        ("code", json!("other")),
        ("stage", json!("arbitrary message")),
        ("kind", json!("arbitrary exception")),
        ("status", json!("arbitrary RPC detail")),
        ("binding_contract_equal", json!("arbitrary value")),
        ("token", json!("untrusted secret")),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(safe_diagnostic(&invalid.to_string()).is_none());
    }
    for field in ["version", "code", "stage", "kind", "status"] {
        let mut invalid = valid.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(safe_diagnostic(&invalid.to_string()).is_none());
    }
    assert!(safe_diagnostic(&" ".repeat(1025)).is_none());
}

async fn freshness(scope: &ConsumerScope<'_>, label: &str) {
    let Ok(c) = scope.app.db.connect().await else {
        println!("M4 OCI freshness: database_unavailable");
        return;
    };
    let rows = c.query("SELECT i.id,i.generation,i.phase,p.observed_generation,(EXTRACT(EPOCH FROM (clock_timestamp()-p.observed_at))*1000)::bigint,(EXTRACT(EPOCH FROM (clock_timestamp()-i.last_heartbeat_at))*1000)::bigint,p.availability='ready',p.observed_at>clock_timestamp()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint) FROM instances i JOIN deployments d ON d.id=i.deployment_id JOIN installations n ON n.id=d.installation_id LEFT JOIN provider_records p ON p.instance_id=i.id WHERE i.id=ANY($1) ORDER BY i.id,p.name,p.version LIMIT 16", &[&scope.instances]).await;
    let Ok(rows) = rows else {
        println!("M4 OCI freshness: query_unavailable");
        return;
    };
    for row in rows {
        // phase is a database enum; health/provider-supplied strings are omitted.
        let observed = json!({"consumer":label,"instance":row.get::<_,Uuid>(0),"generation":row.get::<_,i64>(1),"phase":row.get::<_,String>(2),"observed_generation":row.get::<_,Option<i64>>(3),"observation_age_ms":row.get::<_,Option<i64>>(4),"heartbeat_age_ms":row.get::<_,Option<i64>>(5),"provider_ready":row.get::<_,Option<bool>>(6),"observation_fresh":row.get::<_,Option<bool>>(7)});
        println!("M4 OCI before invoke: {observed}");
    }
}

async fn failure_diagnostic(scope: &ConsumerScope<'_>, instance: Uuid) {
    use xelab_kernel::rpc::{self, capability_service_client::CapabilityServiceClient};
    let Ok(c) = scope.app.db.connect().await else {
        println!("M4 OCI failure diagnostic: database_unavailable");
        return;
    };
    let outer = c.query_opt("SELECT q.id,q.owner_id,q.idempotency_key,i.generation,q.state FROM calls q JOIN instances i ON i.id=q.provider_instance_id WHERE q.provider_instance_id=$1 AND NOT EXISTS(SELECT 1 FROM host_child_calls h WHERE h.child_call_id=q.id) ORDER BY q.created_at DESC,q.id DESC LIMIT 1", &[&instance]).await;
    let Ok(Some(outer)) = outer else {
        println!("M4 OCI failure diagnostic: outer_call_unavailable");
        return;
    };
    let call: Uuid = outer.get(0);
    println!(
        "M4 OCI failed outer: {}",
        json!({"call":call,"instance":instance,"state":outer.get::<_,String>(4)})
    );
    if let Ok(row) = c.query_one("SELECT (SELECT count(*) FROM events WHERE topic=$1),(SELECT count(*) FROM event_dedupe d JOIN events e ON e.id=d.event_id WHERE e.topic=$1),(SELECT count(*) FROM outbox o JOIN events e ON e.id=o.aggregate_id WHERE e.topic=$1 AND o.topic='kernel.event.publish'),(SELECT count(*) FROM event_deliveries WHERE subscription_id=$2),(SELECT count(*) FROM host_call_tickets WHERE parent_call_id=$3),(SELECT count(*) FROM host_child_calls WHERE parent_call_id=$3)", &[&scope.topic,&scope.subscription,&call]).await {
        println!("M4 OCI failure counts: {}",json!({"events":row.get::<_,i64>(0),"dedupe":row.get::<_,i64>(1),"outbox":row.get::<_,i64>(2),"deliveries":row.get::<_,i64>(3),"tickets":row.get::<_,i64>(4),"children":row.get::<_,i64>(5)}));
    }
    if let Ok(rows) = c.query("SELECT id,instance_id=$2,expected_skill IS NOT NULL,provider_deployment_id=$3,dependency_binding_id IS NOT NULL,expires_at>clock_timestamp(),octet_length(delegation_digest)=32,octet_length(verification_digest)=32,length(fingerprint)>0,length(input_digest)>0,requested_call_id IS NOT NULL FROM host_call_tickets WHERE parent_call_id=$1 ORDER BY id LIMIT 8", &[&call,&instance,&scope.provider]).await {
        for row in rows {
            println!("M4 OCI failure ticket: {}",json!({"ticket":row.get::<_,Uuid>(0),"original_consumer_instance":row.get::<_,bool>(1),"skill":row.get::<_,bool>(2),"original_provider_deployment":row.get::<_,Option<bool>>(3),"dependency_binding_present":row.get::<_,bool>(4),"ticket_live":row.get::<_,bool>(5),"delegation_proof_shape":row.get::<_,bool>(6),"verification_proof_shape":row.get::<_,bool>(7),"fingerprint_present":row.get::<_,bool>(8),"input_proof_present":row.get::<_,bool>(9),"requested_call_present":row.get::<_,bool>(10)}));
        }
    }
    if let Ok(rows) = c.query("SELECT q.id,q.state,q.provider_instance_id=$2,d.installation_id=$3,q.skill_id IS NOT NULL,t.id IS NOT NULL,t.dependency_binding_id IS NOT NULL,t.expires_at>clock_timestamp(),octet_length(t.delegation_digest)=32,octet_length(t.verification_digest)=32,length(t.fingerprint)>0,length(t.input_digest)>0 FROM calls q JOIN instances i ON i.id=q.provider_instance_id JOIN deployments d ON d.id=i.deployment_id JOIN host_child_calls h ON h.child_call_id=q.id LEFT JOIN host_call_tickets t ON t.id=h.ticket_id WHERE h.parent_call_id=$1 ORDER BY q.created_at,q.id LIMIT 8", &[&call,&scope.provider_instance,&scope.provider_installation]).await {
        for row in rows {
            println!("M4 OCI failure child: {}",json!({"call":row.get::<_,Uuid>(0),"state":row.get::<_,String>(1),"original_provider_instance":row.get::<_,bool>(2),"original_provider_installation":row.get::<_,bool>(3),"skill":row.get::<_,bool>(4),"ticket_present":row.get::<_,bool>(5),"dependency_binding_present":row.get::<_,Option<bool>>(6),"ticket_live":row.get::<_,Option<bool>>(7),"delegation_proof_shape":row.get::<_,Option<bool>>(8),"verification_proof_shape":row.get::<_,Option<bool>>(9),"fingerprint_present":row.get::<_,Option<bool>>(10),"input_proof_present":row.get::<_,Option<bool>>(11)}));
        }
    }
    // The stored original business key is required by the authenticated fixture.
    // It and every token/digest/fingerprint remain private even on failure.
    let owner: Uuid = outer.get(1);
    let Some(key) = outer.get::<_, Option<String>>(2) else {
        println!("M4 OCI fixture failure: original_business_key_unavailable");
        return;
    };
    let generation: i64 = outer.get(3);
    let diagnostic = tokio::time::timeout(Duration::from_secs(5), async {
        let address = scope
            .app
            .runtime
            .address(instance)
            .map_err(|_| "transport")?;
        let channel = tls::channel(&scope.app.config, instance, generation, &address)
            .await
            .map_err(|_| "transport")?;
        let mut context = controller::context(Some(owner));
        context.idempotency_key = key.clone();
        context.deadline_unix_ms =
            (chrono::Utc::now() + chrono::Duration::seconds(2)).timestamp_millis();
        let mut request = tonic::Request::new(rpc::QueryResultRequest {
            context: Some(context),
            call_id: call.to_string(),
            idempotency_key: key,
        });
        request.set_timeout(Duration::from_secs(2));
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            CapabilityServiceClient::new(channel)
                .max_decoding_message_size(16384)
                .query_result(request),
        )
        .await;
        match response {
            Ok(Ok(response)) => {
                let safe = response
                    .metadata()
                    .get_bin("xelab-m4-fixture-diag-bin")
                    .and_then(|raw| raw.to_bytes().ok())
                    .filter(|raw| raw.len() <= 1024)
                    .and_then(|raw| std::str::from_utf8(&raw).ok().and_then(safe_diagnostic));
                let response = response.into_inner();
                if response.state == rpc::ExecutionState::Unknown as i32
                    && response.outcome.is_none()
                {
                    if let Some(safe) = safe {
                        println!("M4 OCI fixture failure: {safe}");
                        return Ok(());
                    }
                }
                println!("M4 OCI fixture failure: diagnostic_unavailable");
            }
            Ok(Err(status)) => println!("M4 OCI fixture diagnostic RPC code: {:?}", status.code()),
            Err(_) => println!("M4 OCI fixture diagnostic RPC code: DeadlineExceeded"),
        }
        Ok::<(), &'static str>(())
    })
    .await;
    if !matches!(diagnostic, Ok(Ok(()))) {
        println!("M4 OCI fixture failure: diagnostic_transport_unavailable");
    }
}

async fn deploy(
    app: &App,
    router: &Router,
    token: &str,
    installation: Uuid,
    name: &str,
    approval: Value,
) -> (Uuid, Uuid) {
    let c = app.db.connect().await.unwrap();
    let limits: Value = c
        .query_one(
            "SELECT manifest->'minimum_resources' FROM installations WHERE id=$1",
            &[&installation],
        )
        .await
        .unwrap()
        .get(0);
    let created = api(router,"createDeployment",&[],Some(token),json!({"installation_id":installation,"display_name":format!("actual M4 {name}"),"enabled":false,"limits":limits,"config":{},"volume_ids":[]}),&format!("m4-{name}-create"),None).await;
    assert_eq!(created.0, 202, "{}", created.1);
    let deployment = contracts::id(
        &operation(router, created.1["id"].as_str().unwrap(), token).await,
        "id",
    )
    .unwrap();
    let params = [("deployment_id", deployment.to_string())];
    let approved = api(
        router,
        "approvePlugin",
        &params,
        Some(token),
        approval,
        &format!("m4-{name}-approve"),
        Some(1),
    )
    .await;
    assert_eq!(approved.0, 200, "{}", approved.1);
    let current = api(
        router,
        "getDeployment",
        &params,
        Some(token),
        Value::Null,
        "m4-read-before-enable",
        None,
    )
    .await;
    assert_eq!(current.0, 200, "{}", current.1);
    let enabled = api(
        router,
        "setDeploymentEnabled",
        &params,
        Some(token),
        json!({"enabled":true}),
        &format!("m4-{name}-enable"),
        current.1["revision"].as_i64(),
    )
    .await;
    assert_eq!(enabled.0, 202, "{}", enabled.1);
    operation(router, enabled.1["id"].as_str().unwrap(), token).await;
    let c = app.db.connect().await.unwrap();
    let rows = c
        .query(
            "SELECT id FROM instances WHERE deployment_id=$1 AND phase='ready'",
            &[&deployment],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one real admitted instance");
    let instance = rows[0].get(0);
    assert!(app.runtime.running(instance).await.unwrap());
    (deployment, instance)
}

async fn budget(app: &App, instances: &[Uuid], count: i64, cpu: i64) {
    let c = app.db.connect().await.unwrap();
    let reservations = c.query_one("SELECT count(*),COALESCE(sum(cpu_millis),0)::bigint,COALESCE(sum(memory_bytes),0)::bigint FROM reservations WHERE instance_id=ANY($1) AND kind='instance' AND status<>'released'", &[&instances]).await.unwrap();
    assert_eq!(reservations.get::<_, i64>(0), count);
    assert_eq!(reservations.get::<_, i64>(1), cpu);
    let node = c
        .query_one(
            "SELECT reserved_cpu_millis,reserved_memory_bytes FROM node_budget",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(node.get::<_, i64>(0), cpu);
    assert_eq!(node.get::<_, i64>(1), reservations.get::<_, i64>(2));
}

async fn assert_event(scope: &ConsumerScope<'_>, event: Uuid, message: &str, total: i64) {
    let c = scope.app.db.connect().await.unwrap();
    let committed = c.query_one("SELECT e.published,(SELECT count(*) FROM events WHERE topic=$2),(SELECT count(*) FROM event_dedupe WHERE event_id=e.id),(SELECT count(*) FROM outbox WHERE aggregate_id=e.id AND topic='kernel.event.publish' AND delivered_at IS NOT NULL),(SELECT count(*) FROM event_deliveries WHERE subscription_id=$3 AND event_id=e.id AND state='acked'),(SELECT cursor FROM subscriptions WHERE id=$3),e.payload,(SELECT cursor_sequence FROM subscriptions WHERE id=$3)=e.sequence FROM events e WHERE e.id=$1", &[&event,&scope.topic,&scope.subscription]).await.unwrap();
    assert!(committed.get::<_, bool>(0));
    assert_eq!(committed.get::<_, i64>(1), total);
    for index in 2..=4 {
        assert_eq!(committed.get::<_, i64>(index), 1);
    }
    assert_eq!(committed.get::<_, Option<Uuid>>(5), Some(event));
    assert_eq!(committed.get::<_, Value>(6), json!({"message":message}));
    assert!(committed.get::<_, Option<bool>>(7).unwrap());
}

async fn invoke_consumer(
    scope: &ConsumerScope<'_>,
    consumer: Uuid,
    instance: Uuid,
    label: &str,
    message: &str,
    key: &str,
    total: i64,
) -> ConsumerReceipt {
    let input = json!({"contract":scope.invocation_contract,"input":{"message":message,"skill_id":scope.skill,"subscription_id":scope.subscription},"provider_deployment_id":consumer,"timeout_ms":60000});
    freshness(scope, label).await;
    let invoked = api(
        scope.router,
        "invokeCapability",
        &[],
        Some(scope.token),
        input.clone(),
        key,
        None,
    )
    .await;
    if invoked.0 != 200 {
        failure_diagnostic(scope, instance).await;
    }
    assert_eq!(invoked.0, 200, "{}", invoked.1);
    let output = &invoked.1["output"];
    if output["message"] != message
        || output["provider_deployment_id"] != json!(scope.provider)
        || output["duplicate"] != true
        || output["acked"] != true
        || output["event_id"] != output["delivery_event_id"]
    {
        failure_diagnostic(scope, instance).await;
    }
    assert_eq!(output["message"], message);
    assert_eq!(output["provider_deployment_id"], json!(scope.provider));
    assert_eq!(output["duplicate"], true);
    assert_eq!(output["acked"], true);
    assert_eq!(output["event_id"], output["delivery_event_id"]);
    let call = contracts::id(&invoked.1, "call_id").unwrap();
    let children = [
        contracts::id(output, "dependency_call_id").unwrap(),
        contracts::id(output, "skill_call_id").unwrap(),
    ];
    assert_ne!(children[0], children[1]);
    let c = scope.app.db.connect().await.unwrap();
    let rows = c.query("SELECT q.id,q.skill_id,q.provider_instance_id,q.state,d.installation_id,i.deployment_id,h.parent_call_id,t.expected_contract,t.instance_id,t.provider_deployment_id,t.dependency_binding_id,t.parent_call_id FROM host_child_calls h JOIN calls q ON q.id=h.child_call_id JOIN instances i ON i.id=q.provider_instance_id JOIN deployments d ON d.id=i.deployment_id JOIN host_call_tickets t ON t.id=h.ticket_id WHERE h.parent_call_id=$1 ORDER BY q.id", &[&call]).await.unwrap();
    assert_eq!(
        rows.len(),
        2,
        "each consumer must execute both proxies as real child Calls"
    );
    for row in &rows {
        let child: Uuid = row.get(0);
        assert!(children.contains(&child));
        assert_eq!(row.get::<_, Uuid>(2), scope.provider_instance);
        assert_eq!(row.get::<_, String>(3), "succeeded");
        assert_eq!(row.get::<_, Uuid>(4), scope.provider_installation);
        assert_eq!(row.get::<_, Uuid>(5), scope.provider);
        // Host ancestry is the constrained relation and its issuing ticket;
        // the generic nullable calls.parent_call_id is not this M4 contract.
        assert_eq!(row.get::<_, Uuid>(6), call);
        assert_eq!(row.get::<_, Uuid>(11), call);
        assert_eq!(
            row.get::<_, Option<Value>>(7).as_ref(),
            Some(scope.dependency_contract)
        );
        assert_eq!(row.get::<_, Uuid>(8), instance);
        if child == children[1] {
            assert_eq!(row.get::<_, Option<Uuid>>(1), Some(scope.skill));
        } else {
            assert_eq!(row.get::<_, Option<Uuid>>(1), None);
            assert_eq!(row.get::<_, Option<Uuid>>(9), Some(scope.provider));
            assert!(row.get::<_, Option<Uuid>>(10).is_some());
        }
    }
    let event = contracts::id(output, "event_id").unwrap();
    assert_event(scope, event, message, total).await;
    let replay = api(
        scope.router,
        "invokeCapability",
        &[],
        Some(scope.token),
        input,
        key,
        None,
    )
    .await;
    if replay != invoked {
        failure_diagnostic(scope, instance).await;
    }
    assert_eq!(replay, invoked, "confirmed outer replay must not republish");
    assert_event(scope, event, message, total).await;
    let counts = c.query_one("SELECT (SELECT count(*) FROM calls q WHERE q.provider_instance_id=$1 AND NOT EXISTS(SELECT 1 FROM host_child_calls h WHERE h.child_call_id=q.id)),(SELECT count(*) FROM host_child_calls WHERE parent_call_id=$2)", &[&instance,&call]).await.unwrap();
    assert_eq!(
        counts.get::<_, i64>(0),
        1,
        "replay must reuse the original parent Call"
    );
    assert_eq!(
        counts.get::<_, i64>(1),
        2,
        "replay must not execute either child twice"
    );
    budget(scope.app, scope.instances, 3, 300).await;
    ConsumerReceipt {
        call,
        children,
        event,
    }
}

pub(super) async fn run(
    app: &Arc<App>,
    router: &Router,
    token: &str,
    publisher: Uuid,
    signing: &SigningKey,
    provider_installation: Uuid,
    skill: Uuid,
) {
    let c = app.db.connect().await.unwrap();
    let fixed_source = c
        .query_one(
            "SELECT s.installation_id,n.plugin_id FROM skills s JOIN installations n ON n.id=s.installation_id WHERE s.id=$1",
            &[&skill],
        )
        .await
        .unwrap();
    assert_eq!(
        fixed_source.get::<_, Uuid>("installation_id"),
        provider_installation,
        "M4 must deploy the immutable installation that owns the fixed Skill"
    );
    assert_eq!(fixed_source.get::<_, String>("plugin_id"), "org.xelab.echo");
    assert_eq!(
        c.query_one("SELECT reserved_cpu_millis FROM node_budget", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0,
        "earlier OCI scenarios must release their own resources first"
    );
    let (provider, provider_instance) = deploy(
        app,
        router,
        token,
        provider_installation,
        "provider",
        json!({"rules":[],"egress":[]}),
    )
    .await;
    let providers=c.query("SELECT i.id,d.installation_id,n.plugin_id FROM provider_records p JOIN instances i ON i.id=p.instance_id JOIN deployments d ON d.id=i.deployment_id JOIN installations n ON n.id=d.installation_id WHERE p.name='org.xelab.echo' AND p.version='1.0.0' AND p.availability='ready' AND i.phase='ready' AND d.spec->>'enabled'='true' ORDER BY i.id", &[]).await.unwrap();
    assert_eq!(
        providers
            .iter()
            .map(|row| row.get::<_, Uuid>(0))
            .collect::<Vec<_>>(),
        vec![provider_instance],
        "the signed Rust deployment must be the unique live echo provider"
    );
    assert_eq!(
        providers[0].get::<_, Uuid>("installation_id"),
        provider_installation
    );
    assert_eq!(providers[0].get::<_, String>("plugin_id"), "org.xelab.echo");
    let raw = fs::read("examples/generated-m4-consumer/xelab/plugin.json").unwrap();
    let manifest: Value = serde_json::from_slice(&raw).unwrap();
    let image = std::env::var("XELAB_TEST_M4_CONSUMER_IMAGE")
        .expect("CI must build and import the actual M4 consumer image");
    let envelope =
        artifact::signature_envelope(&manifest, artifact::image_ref(&image).unwrap(), &raw)
            .unwrap();
    let installed = api(router,"installPlugin",&[],Some(token),json!({"publisher_id":publisher,"image":image,"signature":STANDARD.encode(signing.sign(envelope.as_bytes()).to_bytes())}),"m4-consumer-install",None).await;
    assert_eq!(installed.0, 202, "{}", installed.1);
    let installation = contracts::id(
        &operation(router, installed.1["id"].as_str().unwrap(), token).await,
        "installation_id",
    )
    .unwrap();
    let rules: Vec<Value> = manifest["requested_rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["rule"].clone())
        .collect();
    let (consumer, consumer_instance) = deploy(
        app,
        router,
        token,
        installation,
        "consumer",
        json!({"rules":rules.clone(),"egress":[]}),
    )
    .await;
    let (shared_consumer, shared_instance) = deploy(
        app,
        router,
        token,
        installation,
        "consumer-shared",
        json!({"rules":rules,"egress":[]}),
    )
    .await;
    let consumers = [consumer_instance, shared_instance];
    let instances = [provider_instance, consumer_instance, shared_instance];
    let provider_process = stream_process(app, provider_instance).await;
    let consumer_process = stream_process(app, consumer_instance).await;
    let shared_process = stream_process(app, shared_instance).await;
    budget(app, &instances, 3, 300).await;
    let base=c.query_one("SELECT provider_deployment_id,provider_installation_id,contract FROM dependency_bindings WHERE consumer_deployment_id=$1 AND bound_call IS NULL", &[&consumer]).await.unwrap();
    let shared_base: Uuid = c.query_one("SELECT provider_deployment_id FROM dependency_bindings WHERE consumer_deployment_id=$1 AND bound_call IS NULL", &[&shared_consumer]).await.unwrap().get(0);
    assert_eq!(
        shared_base, provider,
        "two independent consumers must share the original provider"
    );
    assert_eq!(base.get::<_, Uuid>(0), provider);
    assert_eq!(base.get::<_, Uuid>(1), provider_installation);
    let actual_shared=c.query_one("SELECT b.provider_installation_id,i.id FROM dependency_bindings b JOIN instances i ON i.deployment_id=b.provider_deployment_id AND i.generation=b.provider_generation AND i.phase='ready' WHERE b.consumer_deployment_id=$1 AND b.bound_call IS NULL", &[&shared_consumer]).await.unwrap();
    assert_eq!(actual_shared.get::<_, Uuid>(0), provider_installation);
    assert_eq!(actual_shared.get::<_, Uuid>(1), provider_instance);
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM reservations WHERE instance_id=$1 AND status<>'released'",
            &[&provider_instance]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );

    let declared: Value = serde_json::from_slice(
        &fs::read("examples/generated-m4-consumer/xelab/dependency-contract.json").unwrap(),
    )
    .unwrap();
    assert_eq!(base.get::<_, Value>(2), declared);
    let topic = manifest["event_topics"][0]["topic"].as_str().unwrap();
    let subscription = api(router,"createSubscription",&[],Some(token),json!({"topic":topic,"name":"actual signed OCI M4 consumer","start":"earliest","maximum_attempts":3}),"m4-subscription-create",None).await;
    assert_eq!(subscription.0, 201, "{}", subscription.1);
    let subscription_id = contracts::id(&subscription.1, "id").unwrap();
    let host = tokio::spawn(xelab_kernel::host::serve(app.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!host.is_finished(), "the actual mTLS HostService must bind");
    let scope = ConsumerScope {
        app,
        router,
        token,
        provider,
        provider_installation,
        provider_instance,
        skill,
        subscription: subscription_id,
        topic,
        invocation_contract: &manifest["capabilities"][0]["contract"],
        dependency_contract: &declared,
        instances: &instances,
    };
    let first = invoke_consumer(
        &scope,
        consumer,
        consumer_instance,
        "consumer",
        "actual OCI dependency, Skill and durable event",
        "m4-consumer-invoke",
        1,
    )
    .await;
    let second = invoke_consumer(
        &scope,
        shared_consumer,
        shared_instance,
        "consumer-shared",
        "actual OCI second consumer shares the fixed provider",
        "m4-consumer-shared-invoke",
        2,
    )
    .await;
    assert_ne!(first.call, second.call);
    assert_ne!(first.event, second.event);
    assert!(first
        .children
        .iter()
        .all(|child| !second.children.contains(child)));
    let totals = c.query_one("SELECT (SELECT count(*) FROM events WHERE topic=$1),(SELECT count(*) FROM event_dedupe d JOIN events e ON e.id=d.event_id WHERE e.topic=$1),(SELECT count(*) FROM outbox o JOIN events e ON e.id=o.aggregate_id WHERE e.topic=$1 AND o.topic='kernel.event.publish' AND o.delivered_at IS NOT NULL),(SELECT count(*) FROM event_deliveries WHERE subscription_id=$2 AND state='acked'),(SELECT sequence FROM events WHERE id=$4)>(SELECT sequence FROM events WHERE id=$3),(SELECT count(*) FROM host_child_calls WHERE parent_call_id=ANY($5))", &[&topic,&subscription_id,&first.event,&second.event,&[first.call,second.call].as_slice()]).await.unwrap();
    for index in 0..=3 {
        assert_eq!(
            totals.get::<_, i64>(index),
            2,
            "both actual consumers must commit exactly one event each"
        );
    }
    assert!(
        totals.get::<_, bool>(4),
        "the shared subscription cursor must advance to the second event"
    );
    assert_eq!(
        totals.get::<_, i64>(5),
        4,
        "each actual consumer must execute exactly two child Calls"
    );
    let provider_params = [("deployment_id", provider.to_string())];
    let disabled =
        disable_after_observation(router, &provider_params, token, "m4-provider-disable").await;
    assert_eq!(disabled.0, 202, "{}", disabled.1);
    operation(router, disabled.1["id"].as_str().unwrap(), token).await;
    let mut paused = false;
    for _ in 0..120 {
        let withdrawn:bool=c.query_one("SELECT NOT EXISTS(SELECT 1 FROM provider_records WHERE instance_id=ANY($1) AND availability='ready')", &[&consumers.as_slice()]).await.unwrap().get(0);
        if withdrawn
            && !app.runtime.running(consumer_instance).await.unwrap()
            && !app.runtime.running(shared_instance).await.unwrap()
            && stream_process_exited(&consumer_process)
            && stream_process_exited(&shared_process)
        {
            paused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        paused,
        "required provider disable must physically pause its consumer"
    );
    assert!(!app.runtime.running(provider_instance).await.unwrap());
    assert!(stream_process_exited(&provider_process));
    // The paused consumer retains their original reservations, never another
    // provider reservation while waiting for its fixed dependency.
    budget(app, &instances, 2, 200).await;
    tokio::time::sleep(Duration::from_millis(2200)).await;
    budget(app, &instances, 2, 200).await;
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM instances WHERE deployment_id=$1",
            &[&consumer]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM reservations WHERE instance_id=$1",
            &[&consumer_instance]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM reservations WHERE instance_id=$1",
            &[&shared_instance]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    for (name, id) in [("consumer", consumer), ("consumer-shared", shared_consumer)] {
        let params = [("deployment_id", id.to_string())];
        let disabled =
            disable_after_observation(router, &params, token, &format!("m4-{name}-disable")).await;
        assert_eq!(disabled.0, 202, "{}", disabled.1);
        operation(router, disabled.1["id"].as_str().unwrap(), token).await;
    }
    budget(app, &instances, 0, 0).await;
    let sub_params = [("subscription_id", subscription_id.to_string())];
    let current = api(
        router,
        "getSubscription",
        &sub_params,
        Some(token),
        Value::Null,
        "m4-subscription-read",
        None,
    )
    .await;
    assert_eq!(current.0, 200, "{}", current.1);
    let removed = api(
        router,
        "deleteSubscription",
        &sub_params,
        Some(token),
        Value::Null,
        "m4-subscription-disable",
        current.1["revision"].as_i64(),
    )
    .await;
    assert_eq!(removed.0, 204, "{}", removed.1);
    host.abort();
    let _ = host.await;
    let evidence = json!({"kind":"M4 actual signed OCI component","required":true,"passed":true,"consumer_installation":installation,"consumer_instance":consumer_instance,"shared_consumer_instance":shared_instance,"provider_installation":provider_installation,"provider_instance":provider_instance,"outer_call":first.call,"child_calls":first.children,"event":first.event,"consumers":[{"instance":consumer_instance,"outer_call":first.call,"child_calls":first.children,"event":first.event},{"instance":shared_instance,"outer_call":second.call,"child_calls":second.children,"event":second.event}],"events":[first.event,second.event],"subscription":subscription_id,"signed_image":image,"checks":["both-consumers-execute-host-calls","fixed-dependency-resolve-invoke","fixed-skill-proxy","shared-original-provider-installation-and-instance","transactional-outbox","publish-dedupe","subscribe-double-ack","shared-subscription-cursor-advances-twice","confirmed-outer-replay-without-republish","required-dependency-physical-pause","one-resource-reservation-per-instance"],"production_acceptance":"pending issue #50; AWS DEFERRED"});
    println!("M4 OCI evidence: {evidence}");
    if let Ok(path) = std::env::var("XELAB_M4_OCI_EVIDENCE") {
        tls::atomic_file(
            std::path::Path::new(&path),
            &serde_json::to_vec_pretty(&evidence).unwrap(),
            0o600,
        )
        .unwrap();
    }
}
