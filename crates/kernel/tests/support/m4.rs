//! Actual signed OCI consumer -> workload mTLS HostService -> fixed provider,
//! Skill and durable event paths. No SQL writes manufacture routing/health.
use super::*;

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
    let providers=c.query("SELECT i.id FROM provider_records p JOIN instances i ON i.id=p.instance_id JOIN deployments d ON d.id=i.deployment_id WHERE p.name='org.xelab.echo' AND p.version='1.0.0' AND p.availability='ready' AND i.phase='ready' AND d.spec->>'enabled'='true' ORDER BY i.id", &[]).await.unwrap();
    assert_eq!(
        providers
            .iter()
            .map(|row| row.get::<_, Uuid>(0))
            .collect::<Vec<_>>(),
        vec![provider_instance],
        "the signed Rust deployment must be the unique live echo provider"
    );
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
    let message = "actual OCI dependency, Skill and durable event";
    let input = json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":message,"skill_id":skill,"subscription_id":subscription_id},"provider_deployment_id":consumer,"timeout_ms":60000});
    let invoked = api(
        router,
        "invokeCapability",
        &[],
        Some(token),
        input.clone(),
        "m4-consumer-invoke",
        None,
    )
    .await;
    assert_eq!(invoked.0, 200, "{}", invoked.1);
    let output = &invoked.1["output"];
    assert_eq!(output["message"], message);
    assert_eq!(output["provider_deployment_id"], json!(provider));
    assert_eq!(output["duplicate"], true);
    assert_eq!(output["acked"], true);
    assert_eq!(output["event_id"], output["delivery_event_id"]);
    let call = contracts::id(&invoked.1, "call_id").unwrap();
    let child_calls = [
        contracts::id(output, "dependency_call_id").unwrap(),
        contracts::id(output, "skill_call_id").unwrap(),
    ];
    assert_ne!(child_calls[0], child_calls[1]);
    let children = c.query("SELECT q.id,q.skill_id,q.provider_instance_id,q.state FROM host_child_calls h JOIN calls q ON q.id=h.child_call_id WHERE h.parent_call_id=$1 ORDER BY q.id", &[&call]).await.unwrap();
    assert_eq!(children.len(), 2, "both proxies must use real child Calls");
    for child in &children {
        assert!(child_calls.contains(&child.get::<_, Uuid>(0)));
        assert_eq!(child.get::<_, Uuid>(2), provider_instance);
        assert_eq!(child.get::<_, String>(3), "succeeded");
        if child.get::<_, Uuid>(0) == child_calls[1] {
            assert_eq!(child.get::<_, Option<Uuid>>(1), Some(skill));
        } else {
            assert_eq!(child.get::<_, Option<Uuid>>(1), None);
        }
    }
    let event = contracts::id(output, "event_id").unwrap();
    let committed = c.query_one("SELECT e.published,(SELECT count(*) FROM events WHERE topic=$2),(SELECT count(*) FROM event_dedupe WHERE event_id=e.id),(SELECT count(*) FROM outbox WHERE aggregate_id=e.id AND topic='kernel.event.publish' AND delivered_at IS NOT NULL),(SELECT count(*) FROM event_deliveries WHERE subscription_id=$3 AND event_id=e.id AND state='acked'),(SELECT cursor FROM subscriptions WHERE id=$3) FROM events e WHERE e.id=$1", &[&event,&topic,&subscription_id]).await.unwrap();
    assert!(committed.get::<_, bool>(0));
    for index in 1..=4 {
        assert_eq!(committed.get::<_, i64>(index), 1);
    }
    assert_eq!(committed.get::<_, Option<Uuid>>(5), Some(event));
    let replay = api(
        router,
        "invokeCapability",
        &[],
        Some(token),
        input,
        "m4-consumer-invoke",
        None,
    )
    .await;
    assert_eq!(replay, invoked, "confirmed outer replay must not republish");
    budget(app, &instances, 3, 300).await;
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
    let evidence = json!({"kind":"M4 actual signed OCI component","required":true,"passed":true,"consumer_installation":installation,"consumer_instance":consumer_instance,"shared_consumer_instance":shared_instance,"provider_installation":provider_installation,"provider_instance":provider_instance,"outer_call":call,"child_calls":child_calls,"event":event,"subscription":subscription_id,"signed_image":image,"checks":["fixed-dependency-resolve-invoke","fixed-skill-proxy","transactional-outbox","publish-dedupe","subscribe-double-ack","required-dependency-physical-pause","one-resource-reservation-per-instance"],"production_acceptance":"pending issue #50; AWS DEFERRED"});
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
