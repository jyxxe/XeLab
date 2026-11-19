//! Claim-time checks on bindings and time limits captured by HTTP task admission.
use super::{login, request, Fixture};
use axum::http::Method;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_postgres::Client;
use uuid::Uuid;
use xelab_kernel::{contracts, task_engine::claim};

async fn publish(f: &Fixture, spec: &Value) -> Uuid {
    let workflow = f.publish(&f.token, spec, "publish").await.expect(201);
    contracts::id(&workflow.body, "id").unwrap()
}

async fn submit(f: &Fixture, db: &Client, workflow: Uuid, token: &str, key: &str) -> Uuid {
    let task = request(
        &f.router,
        Method::POST,
        "/api/platform/v1/tasks",
        Some(token),
        Some(&json!({"workflow_id": workflow, "input": {}})),
        &[("idempotency-key", key)],
    )
    .await
    .expect(201);
    let id = contracts::id(&task.body, "id").unwrap();
    db.query_one("SELECT id FROM task_steps WHERE task_id=$1", &[&id])
        .await
        .unwrap()
        .get(0)
}

async fn deployment(f: &Fixture, db: &Client, id: Uuid) -> Uuid {
    db.query_one(
        "INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at)
         SELECT $1,$2,id,jsonb_build_object('installation_id',id,'enabled',true,
                 'limits',manifest->'minimum_resources'),'ready',now(),now()
         FROM installations WHERE id=$3 RETURNING id",
        &[&id, &f.owner, &f.installation],
    )
    .await
    .unwrap()
    .get(0)
}

async fn assert_unleased(db: &Client, step: Uuid, original: Uuid) {
    let row = db
        .query_one(
            "SELECT s.state,s.fence,s.attempt_count,s.provider_deployment_id,
                t.admission->'steps'->0->>'deployment_id' AS frozen,
                (SELECT count(*) FROM attempts WHERE step_id=s.id) AS attempts
         FROM task_steps s JOIN tasks t ON t.id=s.task_id WHERE s.id=$1",
            &[&step],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>("state"), "ready");
    assert_eq!(row.get::<_, i64>("fence"), 0);
    assert_eq!(row.get::<_, i32>("attempt_count"), 0);
    assert_eq!(row.get::<_, i64>("attempts"), 0);
    assert_eq!(row.get::<_, Uuid>("provider_deployment_id"), original);
    assert_eq!(row.get::<_, String>("frozen"), original.to_string());
}

async fn assert_lease_until(f: &Fixture, db: &Client, step: Uuid, until: DateTime<Utc>) {
    let attempt = claim(&f.db, &f.auth, Uuid::new_v4())
        .await
        .unwrap()
        .expect("live claim");
    assert_eq!(contracts::id(&attempt, "step_id").unwrap(), step);
    let lease: DateTime<Utc> = attempt["lease_expires_at"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let issued: DateTime<Utc> = attempt["created_at"].as_str().unwrap().parse().unwrap();
    assert!(issued < lease);
    assert_eq!(lease, until);
    let id = contracts::id(&attempt, "id").unwrap();
    let persisted: DateTime<Utc> = db
        .query_one(
            "SELECT lease_expires_at FROM attempts WHERE id=$1 AND step_id=$2 AND state='leased'",
            &[&id, &step],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(persisted, until);
}

#[tokio::test]
async fn archived_skill_provider_cannot_be_replaced_on_an_admitted_step() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let original = deployment(&f, &db, Uuid::new_v4()).await;
    let skill = request(
        &f.router,
        Method::POST,
        "/api/platform/v1/skills",
        Some(&f.token),
        Some(
            &json!({"installation_id": f.installation, "definition": f.skill_definition,
                     "enabled": true}),
        ),
        &[("idempotency-key", "skill")],
    )
    .await
    .expect(201);
    // The workflow contains only the Skill ID. Admission chooses its sole deployment.
    let mut spec = f.spec("pinned skill");
    spec["steps"][0].as_object_mut().unwrap().remove("contract");
    spec["steps"][0]["skill_id"] = skill.body["id"].clone();
    let workflow = publish(&f, &spec).await;
    let step = submit(&f, &db, workflow, &f.token, "original").await;
    assert_unleased(&db, step, original).await;

    let replacement = deployment(&f, &db, Uuid::new_v4()).await;
    assert_eq!(
        db.execute(
            "UPDATE deployments SET archived_at=now(),phase='disabled',
                spec=jsonb_set(spec,'{enabled}','false') WHERE id=$1",
            &[&original],
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(claim(&f.db, &f.auth, Uuid::new_v4()).await.unwrap(), None);
    assert_unleased(&db, step, original).await;

    // Fresh admission of the same workflow can use the new deployment.
    let fresh = submit(&f, &db, workflow, &f.token, "replacement").await;
    assert_unleased(&db, fresh, replacement).await;
    let attempt = claim(&f.db, &f.auth, Uuid::new_v4())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(contracts::id(&attempt, "step_id").unwrap(), fresh);
    assert_unleased(&db, step, original).await;
}

async fn submit_with_expiring_rule(
    f: &Fixture,
    db: &Client,
    workflow: Uuid,
    seconds: i32,
    key: &str,
) -> (Uuid, DateTime<Utc>) {
    let row = db
        .query_one(
            "UPDATE grants SET expires_at=clock_timestamp()+$2::int*interval '1 second'
         WHERE subject_id=$1 AND scope->>'kind'='capability' RETURNING id,expires_at",
            &[&f.owner, &seconds],
        )
        .await
        .unwrap();
    let grant: Uuid = row.get("id");
    let step = submit(f, db, workflow, &f.token, key).await;
    // Widen the same live grant. Only the proof captured at submission still has this cutoff.
    assert_eq!(
        db.execute("UPDATE grants SET expires_at=NULL WHERE id=$1", &[&grant])
            .await
            .unwrap(),
        1
    );
    (step, row.get("expires_at"))
}

#[tokio::test]
async fn frozen_rule_caps_lease_and_expires_while_claim_waits_on_provider() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let mut blocker = f.db.connect().await.unwrap();
    let uppercase_id = "ABCDEFAB-CDEF-4ABC-8DEF-ABCDEFABCDEF";
    let provider = deployment(&f, &db, Uuid::parse_str(uppercase_id).unwrap()).await;
    let mut spec = f.spec("rule deadline");
    spec["steps"][0]["timeout_ms"] = json!(60_000);
    spec["steps"][0]["provider_deployment_id"] = json!(uppercase_id);
    let workflow = publish(&f, &spec).await;

    let (live, until) = submit_with_expiring_rule(&f, &db, workflow, 20, "live").await;
    assert_unleased(&db, live, provider).await;
    assert_lease_until(&f, &db, live, until).await;
    let (waiting, until) = submit_with_expiring_rule(&f, &db, workflow, 6, "waiting").await;
    let hold = blocker.transaction().await.unwrap();
    let pid: i32 = hold
        .query_one(
            "SELECT pg_backend_pid() FROM deployments WHERE id=$1 FOR UPDATE",
            &[&provider],
        )
        .await
        .unwrap()
        .get(0);
    let worker_db = f.db.clone();
    let auth = f.auth.clone();
    let pending = tokio::spawn(async move { claim(&worker_db, &auth, Uuid::new_v4()).await });

    tokio::time::timeout(Duration::from_secs(8), async {
        let mut live_waiter = None;
        loop {
            let observation = db
                .query_one(
                    "SELECT clock_timestamp() < $2, (SELECT pid FROM pg_stat_activity
                 WHERE datname=current_database() AND wait_event_type='Lock'
                   AND $1=ANY(pg_blocking_pids(pid)))",
                    &[&pid, &until],
                )
                .await
                .unwrap();
            let live: bool = observation.get(0);
            let waiter: Option<i32> = observation.get(1);
            assert!(
                !pending.is_finished(),
                "claim returned while provider was locked"
            );
            if !live {
                assert!(
                    live_waiter.is_some(),
                    "claim must reach the provider before rule expiry"
                );
                assert_eq!(
                    waiter, live_waiter,
                    "same claim must still be waiting at expiry"
                );
                break;
            }
            if waiter.is_some() {
                live_waiter = waiter;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("database clock must pass the frozen rule cutoff");
    hold.rollback().await.unwrap();
    assert_eq!(pending.await.unwrap().unwrap(), None);
    assert_unleased(&db, waiting, provider).await;
}

async fn issuer_key(f: &Fixture, token: &str, name: &str, days: i64) -> Value {
    request(
        &f.router,
        Method::POST,
        &format!("/api/platform/v1/accounts/{}/api-keys", f.owner),
        Some(token),
        Some(&json!({"name":name, "inheritance":{"mode":"follow_parent"},
                     "conditions":{"expires_at":Utc::now()+chrono::Duration::days(days)}})),
        &[("idempotency-key", name)],
    )
    .await
    .expect(201)
    .body
}

#[tokio::test]
async fn surviving_original_grant_is_bounded_by_its_issuers_parent_credential() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let mut spec = f.spec("independent grant sources");
    spec["steps"][0]["timeout_ms"] = json!(60_000);
    let workflow = publish(&f, &spec).await;
    let parent = issuer_key(&f, &f.token, "parent", 2).await;
    let leaf = issuer_key(&f, parent["secret"].as_str().unwrap(), "leaf", 1).await;
    let parent = contracts::id(&parent["credential"], "id").unwrap();
    let leaf = contracts::id(&leaf["credential"], "id").unwrap();

    // A standalone submitter has no inherited capability path from the fixture Owner.
    let subject = Uuid::new_v4();
    db.execute(
        "INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at)
         SELECT $1,'user','active','Claim submitter',password_hash,now(),now() FROM accounts WHERE id=$2",
        &[&subject, &f.owner],
    ).await.unwrap();
    db.execute("INSERT INTO account_closure VALUES($1,$1,0)", &[&subject])
        .await
        .unwrap();
    db.execute(
        "INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at)
         SELECT $1,spec,current_date,now(),now() FROM quotas WHERE account_id=$2",
        &[&subject, &f.owner],
    )
    .await
    .unwrap();
    let withdrawn = Uuid::new_v4();
    let surviving = Uuid::new_v4();
    for (kind, grant, issuer) in [
        ("workflow", Uuid::new_v4(), None),
        ("task", Uuid::new_v4(), None),
        ("capability", withdrawn, None),
        ("capability", surviving, Some(leaf)),
    ] {
        db.execute(
            "INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,
                                expires_at,status,created_at,updated_at)
             VALUES($1,$2,$3,$4,'Owner',jsonb_build_object('kind',$5::text,'match','all'),
                    now()+interval '1 hour','active',now(),now())",
            &[&grant, &subject, &f.owner, &issuer, &kind],
        )
        .await
        .unwrap();
    }
    let token = login(&f.router, subject).await;
    let step = submit(&f, &db, workflow, &token, "two-grants").await;
    let proof: Value = db
        .query_one(
            "SELECT t.admission FROM tasks t JOIN task_steps s ON s.task_id=t.id WHERE s.id=$1",
            &[&step],
        )
        .await
        .unwrap()
        .get(0);
    let paths = proof["steps"][0]["targets"][0]["paths"].as_array().unwrap();
    for (required, other) in [(withdrawn, surviving), (surviving, withdrawn)] {
        assert!(
            paths.iter().any(|p| {
                let sources = p["sources"].as_array().unwrap();
                sources.contains(&json!(required)) && !sources.contains(&json!(other))
            }),
            "both independent alternatives must be frozen"
        );
    }
    let chain = proof["credential_chain"].as_array().unwrap();
    assert!(!chain.contains(&json!(leaf)) && !chain.contains(&json!(parent)));

    db.execute(
        "UPDATE grants SET expires_at=now() WHERE id=$1",
        &[&withdrawn],
    )
    .await
    .unwrap();
    // The leaf and its rule conditions still last an hour or more. Shorten only
    // its parent's physical expiry, independently of the task's own issuer.
    let until: DateTime<Utc> = db
        .query_one(
            "UPDATE credentials SET expires_at=clock_timestamp()+interval '20 seconds'
         WHERE id=$1 RETURNING expires_at",
            &[&parent],
        )
        .await
        .unwrap()
        .get(0);
    assert_lease_until(&f, &db, step, until).await;
}

#[tokio::test]
async fn shortened_ancestor_account_lifetime_caps_an_already_admitted_lease() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let (_, child_token) = f.child().await;
    let mut spec = f.spec("ancestor expiry");
    spec["steps"][0]["timeout_ms"] = json!(60_000);
    let workflow = publish(&f, &spec).await;
    let step = submit(&f, &db, workflow, &child_token, "ancestor").await;

    let until: DateTime<Utc> = db
        .query_one(
            "UPDATE accounts SET expires_at=clock_timestamp()+interval '20 seconds'
         WHERE id=$1 RETURNING expires_at",
            &[&f.owner],
        )
        .await
        .unwrap()
        .get(0);
    assert_lease_until(&f, &db, step, until).await;
}

#[tokio::test]
async fn live_capability_grant_cannot_fill_empty_sources_in_an_admitted_path() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let provider = deployment(&f, &db, Uuid::new_v4()).await;
    let mut spec = f.spec("incomplete proof");
    spec["steps"][0]["provider_deployment_id"] = json!(provider);
    let workflow = publish(&f, &spec).await;
    let step = submit(&f, &db, workflow, &f.token, "incomplete").await;
    let row = db.query_one(
        "SELECT t.id,t.admission FROM tasks t JOIN task_steps s ON s.task_id=t.id WHERE s.id=$1",
        &[&step],
    ).await.unwrap();
    let task: Uuid = row.get("id");
    let mut admission: Value = row.get("admission");
    let target = admission["steps"][0]["targets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|target| target["action"] == "capability.invoke")
        .unwrap();
    let selector = target["resource"].clone();
    let paths = target["paths"].as_array_mut().unwrap();
    assert!(!paths.is_empty());
    for path in paths {
        assert_eq!(path["rule"]["action"], "capability.invoke");
        assert_eq!(path["rule"]["resource"], selector);
        let sources = path["sources"].as_array_mut().unwrap();
        assert!(!sources.is_empty());
        sources.clear();
    }
    assert_eq!(
        db.execute(
            "UPDATE tasks SET admission=$2 WHERE id=$1",
            &[&task, &admission]
        )
        .await
        .unwrap(),
        1
    );

    assert_eq!(claim(&f.db, &f.auth, Uuid::new_v4()).await.unwrap(), None);
    let state: String = db
        .query_one("SELECT state FROM tasks WHERE id=$1", &[&task])
        .await
        .unwrap()
        .get(0);
    assert_eq!(state, "paused_authorization");
    assert_unleased(&db, step, provider).await;

    let fresh = submit(&f, &db, workflow, &f.token, "intact").await;
    let attempt = claim(&f.db, &f.auth, Uuid::new_v4())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(contracts::id(&attempt, "step_id").unwrap(), fresh);
}
