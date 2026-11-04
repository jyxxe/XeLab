//! Ordinary side effects execute once over the actual signed OCI RPC transport.
//! Successful queries preserve the original instance/generation/key; uncertain
//! non-repeatable work is fenced by the existing Controller before quota release.
use super::*;

pub struct Evidence {
    skills: std::collections::BTreeMap<String, String>,
    saved: Vec<Value>,
    unknown: Vec<Uuid>,
    query_successes: usize,
    daily: i64,
    calls: i64,
}

async fn usage(c: &tokio_postgres::Client, owner: Uuid) -> (i64, i64, i64) {
    let row = c.query_one("SELECT (SELECT count(*) FROM calls WHERE owner_id=$1) AS calls,daily_calls_used,concurrent_calls_used FROM quotas WHERE account_id=$1", &[&owner]).await.unwrap();
    (
        row.get("calls"),
        row.get("daily_calls_used"),
        row.get("concurrent_calls_used"),
    )
}

pub async fn successful(
    router: &Router,
    db: &Db,
    owner: Uuid,
    token: &str,
    installation: &Value,
    manifest: &Value,
    deployment: &str,
) -> Evidence {
    let c = db.connect().await.unwrap();
    let before = usage(&c, owner).await;
    assert_eq!(before.2, 0);
    let mut evidence = Evidence {
        skills: Default::default(),
        saved: Vec::new(),
        unknown: Vec::new(),
        query_successes: 0,
        calls: before.0,
        daily: before.1,
    };
    for effect in ["idempotent", "queryable", "non_repeatable"] {
        let definition = manifest["skills"]
            .as_array()
            .unwrap()
            .iter()
            .find(|skill| skill["name"] == format!("org.xelab.echo.{effect}"))
            .unwrap();
        let created = api(
            router,
            "registerSkill",
            &[],
            Some(token),
            json!({"installation_id":installation,"definition":definition,"enabled":true}),
            &format!("effects-register-{effect}"),
            None,
        )
        .await;
        assert_eq!(created.0, 201, "{}", created.1);
        let skill = created.1["id"].as_str().unwrap();
        evidence.skills.insert(effect.into(), skill.into());
        let request = json!({"input":{"message":format!("actual unary {effect}"),"count":2},"provider_deployment_id":deployment,"timeout_ms":10000});
        let key = format!("effects-unary-{effect}");
        let called = api(
            router,
            "invokeSkill",
            &[("skill_id", skill.into())],
            Some(token),
            request.clone(),
            &key,
            None,
        )
        .await;
        assert_eq!(called.0, 200, "{}", called.1);
        contracts::validate("SkillCallResult", &called.1).unwrap();
        assert_eq!(
            called.1["call"]["output"]["message"],
            request["input"]["message"]
        );
        if effect != "idempotent" {
            assert!(
                called.1["call"]["output"]["execution_count"]
                    .as_u64()
                    .unwrap()
                    >= 1
            );
        }
        let replay = api(
            router,
            "invokeSkill",
            &[("skill_id", skill.into())],
            Some(token),
            request.clone(),
            &key,
            None,
        )
        .await;
        assert_eq!(
            replay, called,
            "Only the durable core result may be replayed"
        );
        let mut changed = request.clone();
        changed["input"]["message"] = json!("different intent");
        assert_eq!(
            api(
                router,
                "invokeSkill",
                &[("skill_id", skill.into())],
                Some(token),
                changed,
                &key,
                None
            )
            .await
            .0,
            409
        );
        evidence.saved.push(called.1["call"].clone());
        let response = stream_skill_request(
            router,
            token,
            skill,
            &request,
            &format!("effects-stream-{effect}"),
            false,
        )
        .await;
        assert_eq!(response.status(), 200);
        let events = collect_skill_events(response.into_body()).await;
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["event"]["kind"], "accepted");
        assert_eq!(events[1]["event"]["kind"], "item");
        assert_eq!(events[2]["event"]["kind"], "item");
        assert_eq!(events[3]["event"]["kind"], "completed");
        assert_eq!(
            events[3]["event"]["payload"]["output"]["message"],
            request["input"]["message"]
        );
        if effect != "idempotent" {
            assert!(
                events[3]["event"]["payload"]["output"]["execution_count"]
                    .as_u64()
                    .unwrap()
                    >= 1
            );
        }
        evidence.saved.push(events[3]["event"]["payload"].clone());
        assert_eq!(
            stream_skill_request(
                router,
                token,
                skill,
                &request,
                &format!("effects-stream-{effect}"),
                false
            )
            .await
            .status(),
            409
        );
    }
    let after = usage(&c, owner).await;
    assert_eq!(
        after,
        (before.0 + 6, before.1 + 6, 0),
        "Unary/stream result replay and conflicts cannot execute or charge again"
    );
    evidence.calls = after.0;
    evidence.daily = after.1;
    evidence
}

impl Evidence {
    /// Align fault injection with an observed real health probe. The Controller
    /// continues normal reconciliation and physical fencing throughout; a query
    /// that loses the race must remain unknown, not change generation or retry.
    #[allow(clippy::too_many_arguments)] // Explicit real fixture identities and fixed deployment.
    pub async fn interrupted(
        &mut self,
        app: &App,
        router: &Router,
        db: &Db,
        owner: Uuid,
        token: &str,
        mut instance: Uuid,
        deployment: &str,
    ) -> Uuid {
        let c = db.connect().await.unwrap();
        for effect in ["queryable", "non_repeatable"] {
            let skill = self.skills[effect].clone();
            let variants: &[(bool, bool)] = if effect == "queryable" {
                &[(false, false), (true, false), (false, true)]
            } else {
                &[(false, false), (true, false)]
            };
            for &(stream, cache_loss) in variants {
                let observed: chrono::DateTime<chrono::Utc> = c.query_one("SELECT max(observed_at) FROM provider_records WHERE instance_id=$1 AND availability='ready'", &[&instance]).await.unwrap().get(0);
                let mut fresh = false;
                for _ in 0..120 {
                    let current: bool = c.query_one("SELECT EXISTS(SELECT 1 FROM provider_records WHERE instance_id=$1 AND availability='ready' AND observed_at>$2)", &[&instance,&observed]).await.unwrap().get(0);
                    if current && app.ready.load(Ordering::SeqCst) {
                        fresh = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                assert!(fresh, "An actual health probe must precede each interruption; no synthetic freshness is permitted");
                let old_process = stream_process(app, instance).await;
                let key = format!("effects-after-commit-{effect}-{stream}-{cache_loss}");
                let request = json!({"input":{"message":key,"count":2,"fault":"after-commit"},"provider_deployment_id":deployment,"timeout_ms":10000});
                let call = if stream {
                    let response =
                        stream_skill_request(router, token, &skill, &request, &key, false).await;
                    assert_eq!(response.status(), 200);
                    let events = collect_skill_events(response.into_body()).await;
                    assert_eq!(events.first().unwrap()["event"]["kind"], "accepted");
                    assert_ne!(
                        events.last().unwrap()["event"]["kind"],
                        "completed",
                        "A missing terminal cannot be fabricated"
                    );
                    assert_eq!(
                        stream_skill_request(router, token, &skill, &request, &key, false)
                            .await
                            .status(),
                        409
                    );
                    events[0]["event"]["call_id"]
                        .as_str()
                        .unwrap()
                        .parse::<Uuid>()
                        .unwrap()
                } else {
                    let failed = api(
                        router,
                        "invokeSkill",
                        &[("skill_id", skill.clone())],
                        Some(token),
                        request.clone(),
                        &key,
                        None,
                    )
                    .await;
                    assert_eq!(failed.0, 503, "{}", failed.1);
                    let call = failed.1["error"]["resource_id"]
                        .as_str()
                        .unwrap()
                        .parse::<Uuid>()
                        .unwrap();
                    let replay = api(
                        router,
                        "invokeSkill",
                        &[("skill_id", skill.clone())],
                        Some(token),
                        request.clone(),
                        &key,
                        None,
                    )
                    .await;
                    assert_eq!(replay.0, 409, "{}", replay.1);
                    assert_eq!(replay.1["error"]["resource_id"], call.to_string());
                    call
                };
                let row = c.query_one("SELECT state,provider_instance_id,r.status FROM calls c JOIN reservations r ON r.id=c.id AND r.kind='call' WHERE c.id=$1", &[&call]).await.unwrap();
                assert_eq!(row.get::<_, String>("state"), "unknown");
                assert_eq!(row.get::<_, Uuid>("provider_instance_id"), instance);
                if row.get::<_, String>("status") == "released" {
                    assert!(
                        stream_process_exited(&old_process),
                        "Only physical fencing can release an unknown Call before query"
                    );
                }
                if cache_loss {
                    self.unknown.push(call);
                    let recovery = StreamRecovery {
                        app,
                        router,
                        db,
                        token,
                        skill: &skill,
                    };
                    instance = recovery
                        .fence_and_ready(&[call], instance, &old_process, &key)
                        .await
                        .0;
                    assert!(stream_process_exited(&old_process));
                    let lost = api(
                        router,
                        "getCall",
                        &[("call_id", call.to_string())],
                        Some(token),
                        Value::Null,
                        "effects-queryable-cache-lost-after-restart",
                        None,
                    )
                    .await;
                    assert_eq!(lost.0, 200, "{}", lost.1);
                    assert_eq!(
                        lost.1["state"], "unknown",
                        "A missing old-generation query record cannot prove no effect occurred"
                    );
                    assert!(lost.1["result"].is_null());
                    continue;
                }
                let read = api(
                    router,
                    "getCall",
                    &[("call_id", call.to_string())],
                    Some(token),
                    Value::Null,
                    &format!("effects-query-{call}"),
                    None,
                )
                .await;
                assert_eq!(read.0, 200, "{}", read.1);
                if effect == "queryable" && read.1["state"] == "succeeded" {
                    assert_eq!(
                        read.1["result"]["output"]["message"],
                        request["input"]["message"]
                    );
                    assert!(
                        read.1["result"]["output"]["execution_count"]
                            .as_u64()
                            .unwrap()
                            >= 1
                    );
                    self.saved.push(read.1["result"].clone());
                    self.query_successes += 1;
                    if !stream {
                        let replay = api(
                            router,
                            "invokeSkill",
                            &[("skill_id", skill.clone())],
                            Some(token),
                            request,
                            &key,
                            None,
                        )
                        .await;
                        assert_eq!(replay.0, 200, "{}", replay.1);
                        assert_eq!(replay.1["call"], read.1["result"]);
                    }
                } else {
                    assert_eq!(read.1["state"], "unknown", "An uncertain result cannot be queried/reexecuted against another generation");
                    assert!(read.1["result"].is_null());
                    self.unknown.push(call);
                    let recovery = StreamRecovery {
                        app,
                        router,
                        db,
                        token,
                        skill: &skill,
                    };
                    instance = recovery
                        .fence_and_ready(&[call], instance, &old_process, &key)
                        .await
                        .0;
                    assert!(stream_process_exited(&old_process));
                    let after = api(
                        router,
                        "getCall",
                        &[("call_id", call.to_string())],
                        Some(token),
                        Value::Null,
                        "effects-no-query-after-generation-change",
                        None,
                    )
                    .await;
                    assert_eq!(after.0, 200, "{}", after.1);
                    assert_eq!(
                        after.1["state"], "unknown",
                        "A new generation cannot infer a missing old result"
                    );
                }
            }
        }
        assert!(self.query_successes >= 1, "At least one actual original-provider OCI QueryResult must be observed; metadata-only tests cannot replace this evidence");
        let after = usage(&c, owner).await;
        assert_eq!(
            after,
            (self.calls + 5, self.daily + 5, 0),
            "Queries and physical recovery never create or charge another business execution"
        );
        self.calls = after.0;
        self.daily = after.1;
        instance
    }

    pub async fn after_restart(
        &self,
        app: &App,
        router: &Router,
        db: &Db,
        owner: Uuid,
        token: &str,
        instance: Uuid,
    ) {
        assert!(app.runtime.running(instance).await.unwrap());
        let c = db.connect().await.unwrap();
        for call in &self.unknown {
            let row = c.query_one("SELECT c.state,r.status FROM calls c JOIN reservations r ON r.id=c.id AND r.kind='call' WHERE c.id=$1", &[call]).await.unwrap();
            assert_eq!(row.get::<_, String>("state"), "unknown");
            assert_eq!(row.get::<_, String>("status"), "released");
        }
        for saved in &self.saved {
            let call = saved["call_id"].as_str().unwrap();
            let read = api(
                router,
                "getCall",
                &[("call_id", call.into())],
                Some(token),
                Value::Null,
                "effects-read-persistent",
                None,
            )
            .await;
            assert_eq!(read.0, 200, "{}", read.1);
            assert_eq!(read.1["state"], "succeeded");
            assert_eq!(&read.1["result"], saved);
        }
        // The enclosing baseline makes one ordinary call after restarting.
        assert_eq!(
            usage(&c, owner).await,
            (self.calls + 1, self.daily + 1, 0),
            "Kernel restart and read/query/replay do not create business calls"
        );
        println!("Ordinary effects: actual signed OCI idempotent/queryable/non_repeatable unary and streams, durable result replay, same-key conflicts, committed-response loss, fixed-provider QueryResult, uncertain non-repeatable fencing and restart quota convergence passed");
    }
}
