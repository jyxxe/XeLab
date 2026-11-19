use super::Db;
use crate::{auth, contracts};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio_postgres::Client;
use uuid::Uuid;

const PASSWORD: &str = "correct horse schedule fire fixture";

async fn run_batch(
    c: &mut Client,
    auth: &auth::Auth,
    startup: DateTime<Utc>,
    through: DateTime<Utc>,
) -> crate::error::Result<()> {
    super::run_batch(c, auth, startup, through, &|| false).await
}

struct Fixture {
    db: Db,
    auth: auth::Auth,
    ctx: auth::Context,
    workflow: Uuid,
}

impl Fixture {
    async fn new() -> Option<Self> {
        let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: Schedule trigger requires PostgreSQL");
            return None;
        };
        let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
        let name = format!("schedule_trigger_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let db = Db::new(format!("{base} dbname={name}")).unwrap();
        db.migrate().await.unwrap();
        let mut c = db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let owner = auth::bootstrap(
            &tx,
            "Schedule trigger owner",
            PASSWORD,
            &json!({
                "maximum_depth":4,"maximum_descendants":20,"maximum_api_keys":50,
                "maximum_concurrent_calls":16,"daily_calls":10000
            }),
        )
        .await
        .unwrap();
        let auth = auth::Auth::new(vec![48; 32]).unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        let issued = auth
            .login(&tx, owner, PASSWORD.into(), peer, Uuid::new_v4())
            .await
            .unwrap();
        let ctx = auth
            .authenticate(
                &tx,
                issued["access_token"].as_str().unwrap(),
                peer,
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let publisher = Uuid::new_v4();
        let workflow = Uuid::new_v4();
        let digest = contracts::digest(b"schedule trigger fixture");
        let contract = json!({"name":"tool.echo","version":"1.0.0","schema_digest":digest});
        let manifest = json!({"data_schema_version":"none","capabilities":[{"contract":contract}]});
        tx.execute(
            "INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at)
            VALUES($1,'Schedule fixture','schedule-fixture','fixture','trusted',now(),now())",
            &[&publisher],
        )
        .await
        .unwrap();
        tx.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,
            manifest_digest,manifest,state,created_at,updated_at)
            VALUES($1,'org.xelab.schedule-fixture','1.0.0',$2,'fixture','fixture',$3,'installed',now(),now())",
            &[&Uuid::new_v4(),&publisher,&manifest]).await.unwrap();
        tx.execute(
            "INSERT INTO capability_contracts(name,version,schema_digest,spec)
            VALUES('tool.echo','1.0.0',$1,$2)",
            &[&digest, &json!({"contract":contract})],
        )
        .await
        .unwrap();
        let spec = json!({"name":"scheduled workflow","input_schema":{"type":"object"},
          "steps":[{"id":"first","depends_on":[],"timeout_ms":120,"maximum_attempts":2,
                    "input_mapping":{},"contract":contract},
                   {"id":"second","depends_on":["first"],"timeout_ms":120,"input_mapping":{},"contract":contract}]});
        tx.execute(
            "INSERT INTO workflows(id,owner_id,name,version,spec,status,created_at,updated_at)
            VALUES($1,$2,'scheduled workflow',1,$3,'active',now(),now())",
            &[&workflow, &owner, &spec],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        Some(Self {
            db,
            auth,
            ctx,
            workflow,
        })
    }

    async fn schedule(
        &self,
        c: &mut Client,
        at: DateTime<Utc>,
        cron: &str,
        zone: &str,
        misfire: &str,
        overlap: &str,
    ) -> Uuid {
        let tx = c.transaction().await.unwrap();
        crate::db::auth_lock(&tx, false).await.unwrap();
        let (record, _) = crate::schedules::create(
            &tx,
            &self.ctx,
            &json!({
                "name":"dispatched schedule","workflow_id":self.workflow,"cron":cron,
                "timezone":zone,"misfire":misfire,"overlap":overlap,"input":{"original":true}
            }),
        )
        .await
        .unwrap();
        let id = contracts::id(&record, "id").unwrap();
        tx.execute(
            "UPDATE schedules SET next_fire_at=$2 WHERE id=$1",
            &[&id, &at],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        id
    }

    async fn app(&self, ready: bool) -> Arc<crate::api::App> {
        let source: toml::Value = toml::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/contracts/examples/kernel.toml",
        )))
        .unwrap();
        let config =
            crate::config::Config::from_value(serde_json::to_value(source).unwrap()).unwrap();
        Arc::new(crate::api::App {
            db: self.db.clone(),
            auth: self.auth.clone(),
            leader: Arc::new(self.db.leader().await.unwrap()),
            runtime: crate::runtime::Runtime::new(config.clone()),
            config,
            ready: AtomicBool::new(ready),
            shutdown: AtomicBool::new(false),
            blob_uploads: tokio::sync::Semaphore::new(4),
        })
    }
}

fn time(raw: &str) -> DateTime<Utc> {
    raw.parse().unwrap()
}

async fn fires(c: &Client, schedule: Uuid) -> Vec<(DateTime<Utc>, String, Option<Uuid>)> {
    c.query("SELECT scheduled_at,outcome,task_id FROM schedule_fires WHERE schedule_id=$1 ORDER BY scheduled_at",
        &[&schedule]).await.unwrap().iter().map(|row| (row.get(0),row.get(1),row.get(2))).collect()
}

async fn cursor(c: &Client, schedule: Uuid) -> DateTime<Utc> {
    c.query_one(
        "SELECT next_fire_at FROM schedules WHERE id=$1",
        &[&schedule],
    )
    .await
    .unwrap()
    .get(0)
}

#[tokio::test]
async fn concurrent_due_ticks_deduplicate_and_persist_original_task_steps_and_cursor() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut a = f.db.connect().await.unwrap();
    let mut b = f.db.connect().await.unwrap();
    let at = time("2025-03-01T09:00:00Z");
    let startup = at - Duration::seconds(1);
    let schedule = f
        .schedule(&mut a, at, "*/5 * * * *", "UTC", "skip", "skip")
        .await;
    let (first, second) = tokio::join!(
        run_batch(&mut a, &f.auth, startup, at),
        run_batch(&mut b, &f.auth, startup, at),
    );
    first.unwrap();
    second.unwrap();
    let ledger = fires(&a, schedule).await;
    assert_eq!(ledger.len(), 1);
    assert_eq!((ledger[0].0, ledger[0].1.as_str()), (at, "created"));
    let task = ledger[0].2.unwrap();
    assert_eq!(cursor(&a, schedule).await, at + Duration::minutes(5));
    let r = a.query_one("SELECT t.workflow_id,t.owner_id,t.input,
            t.admission=s.admission->'task_admission', t.delegation_id<>s.delegation_id,
            child.issuer_credential_id=original.issuer_credential_id,
            child.expires_at<=least(original.expires_at,t.deadline_at),
            child.maximum_rules=original.maximum_rules
          FROM tasks t JOIN execution_delegations child ON child.id=t.delegation_id
          JOIN schedules s ON s.id=$2 JOIN execution_delegations original ON original.id=s.delegation_id
         WHERE t.id=$1",&[&task,&schedule]).await.unwrap();
    assert_eq!(r.get::<_, Uuid>(0), f.workflow);
    assert_eq!(r.get::<_, Uuid>(1), f.ctx.subject);
    assert_eq!(r.get::<_, Value>(2), json!({"original":true}));
    assert!((3..8).all(|index| r.get::<_, bool>(index)));
    let steps: Vec<String> = a
        .query(
            "SELECT state FROM task_steps WHERE task_id=$1 ORDER BY step_key",
            &[&task],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(steps, ["ready", "pending"]);
    let attempt = crate::task_engine::claim(&f.db, &f.auth, Uuid::new_v4())
        .await
        .unwrap()
        .unwrap();
    let step = contracts::id(&attempt, "step_id").unwrap();
    assert_eq!(
        a.query_one("SELECT task_id FROM task_steps WHERE id=$1", &[&step])
            .await
            .unwrap()
            .get::<_, Uuid>(0),
        task
    );
    a.execute(
        "UPDATE schedules SET next_fire_at=$2 WHERE id=$1",
        &[&schedule, &at],
    )
    .await
    .unwrap();
    run_batch(&mut a, &f.auth, startup, at).await.unwrap();
    assert_eq!(fires(&a, schedule).await, ledger);
    let audit = a.query_one("SELECT subject_id,credential_id,reason_code FROM audit_events
         WHERE action='schedule.fire' AND outcome='succeeded'
         AND resource=jsonb_build_object('kind','schedule','match','ids','ids',jsonb_build_array($1::uuid))",
        &[&schedule]).await.unwrap();
    assert_eq!(audit.get::<_, Uuid>(0), f.ctx.subject);
    assert_eq!(audit.get::<_, Uuid>(1), f.ctx.credential);
    assert_eq!(audit.get::<_, String>(2), "TASK_SUBMITTED");
    assert_eq!(
        a.query_one("SELECT count(*) FROM tasks", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}

#[tokio::test]
async fn startup_misfire_distinguishes_skip_and_run_once_and_keeps_first_dst_fold() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.db.connect().await.unwrap();
    let first = time("2025-11-01T05:30:00Z");
    let folded = time("2025-11-02T05:30:00Z");
    let startup = time("2025-11-02T07:00:00Z");
    let next = time("2025-11-03T06:30:00Z");
    let skip = f
        .schedule(
            &mut c,
            first,
            "30 1 * * *",
            "America/New_York",
            "skip",
            "skip",
        )
        .await;
    let run_once = f
        .schedule(
            &mut c,
            first,
            "30 1 * * *",
            "America/New_York",
            "run_once",
            "skip",
        )
        .await;
    run_batch(&mut c, &f.auth, startup, startup).await.unwrap();
    assert_eq!(
        fires(&c, skip).await,
        [(folded, "skipped".to_owned(), None)]
    );
    let run = fires(&c, run_once).await;
    assert_eq!(run.len(), 1);
    assert_eq!((run[0].0, run[0].1.as_str()), (folded, "created"));
    assert!(run[0].2.is_some());
    assert_eq!(cursor(&c, skip).await, next);
    assert_eq!(cursor(&c, run_once).await, next);
    run_batch(&mut c, &f.auth, startup, next).await.unwrap();
    assert_eq!(fires(&c, skip).await[1].1, "created");
    assert_eq!(
        fires(&c, run_once).await[1],
        (next, "skipped".to_owned(), None)
    );
}

#[tokio::test]
async fn queue_one_keeps_latest_due_while_unknown_and_promotes_before_next_cron() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.db.connect().await.unwrap();
    let first = time("2025-08-01T05:30:00Z");
    let startup = first - Duration::seconds(1);
    let schedule = f
        .schedule(&mut c, first, "* * * * *", "UTC", "skip", "queue_one")
        .await;
    run_batch(&mut c, &f.auth, startup, first).await.unwrap();
    let first_task = fires(&c, schedule).await[0].2.unwrap();
    c.execute(
        "UPDATE tasks SET state='unknown' WHERE id=$1",
        &[&first_task],
    )
    .await
    .unwrap();
    let older = first + Duration::minutes(1);
    let latest = first + Duration::minutes(2);
    for through in [older, latest] {
        run_batch(&mut c, &f.auth, startup, through).await.unwrap();
    }
    let held = fires(&c, schedule).await;
    assert_eq!(held[1], (older, "skipped".to_owned(), None));
    assert_eq!(held[2], (latest, "pending".to_owned(), None));
    c.execute(
        "UPDATE tasks SET state='failed' WHERE id=$1",
        &[&first_task],
    )
    .await
    .unwrap();
    let next = cursor(&c, schedule).await;
    run_batch(&mut c, &f.auth, startup, latest).await.unwrap();
    let settled = fires(&c, schedule).await;
    assert_eq!(settled.len(), 3);
    assert_eq!(settled[2].1, "created");
    assert_ne!(settled[2].2, Some(first_task));
    assert!(settled[2].2.is_some());
    assert_eq!(cursor(&c, schedule).await, next);
}

#[tokio::test]
async fn replacement_for_original_source_and_archived_workflow_are_paused_without_task() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.db.connect().await.unwrap();
    let at = time("2025-03-01T09:00:00Z");
    let startup = at - Duration::seconds(1);
    let s = f
        .schedule(&mut c, at, "* * * * *", "UTC", "skip", "skip")
        .await;
    let archived = f
        .schedule(
            &mut c,
            at + Duration::minutes(1),
            "* * * * *",
            "UTC",
            "skip",
            "skip",
        )
        .await;
    let row = c
        .query_one("SELECT admission FROM schedules WHERE id=$1", &[&s])
        .await
        .unwrap();
    let proof: Value = row.get(0);
    let grant: Uuid = proof["task_create_paths"][0]["sources"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // Keep an equivalent current source and detach the original one into
    // another independent account. Its global grant expansion stays live.
    c.execute(
        "INSERT INTO grants(id,subject_id,issuer_id,role_id,scope,status,created_at,updated_at)
         SELECT $2,subject_id,issuer_id,role_id,scope,'active',created_at,now()
           FROM grants WHERE id=$1",
        &[&grant, &Uuid::new_v4()],
    )
    .await
    .unwrap();
    let detached_owner = Uuid::new_v4();
    c.execute(
        "INSERT INTO accounts(id,kind,status,display_name,independent_owner,created_at,updated_at)
         VALUES($1,'user','active','Detached fixture owner',true,now(),now())",
        &[&detached_owner],
    )
    .await
    .unwrap();
    c.execute(
        "INSERT INTO account_closure VALUES($1,$1,0)",
        &[&detached_owner],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE grants g SET subject_id=a.id,issuer_id=a.id,created_at=a.created_at,updated_at=now()
          FROM accounts a WHERE g.id=$1 AND a.id=$2",
        &[&grant, &detached_owner],
    ).await.unwrap();
    assert!(!crate::policy::Policy::for_context(&c, &f.ctx)
        .grant(grant, 1)
        .await
        .unwrap()
        .is_empty());
    auth::authorize(
        &c,
        &f.ctx,
        "task.create",
        &auth::Resource::new("task", "new", Some(f.ctx.subject)),
    )
    .await
    .unwrap();
    run_batch(&mut c, &f.auth, startup, at).await.unwrap();
    assert_eq!(fires(&c, s).await, [(at, "paused".to_owned(), None)]);
    c.execute(
        "UPDATE workflows SET status='archived' WHERE id=$1",
        &[&f.workflow],
    )
    .await
    .unwrap();
    run_batch(&mut c, &f.auth, startup, at + Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(
        fires(&c, archived).await,
        [(at + Duration::minutes(1), "paused".to_owned(), None)]
    );
    assert_eq!(
        c.query_one("SELECT count(*) FROM tasks", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn task_and_cursor_roll_back_on_fire_failure_and_gate_holds_offline_recovery() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.db.connect().await.unwrap();
    let at = time("2025-03-01T09:00:00Z");
    let start = at - Duration::seconds(1);
    let schedule = f
        .schedule(&mut c, at, "* * * * *", "UTC", "skip", "skip")
        .await;
    c.batch_execute(
        "ALTER TABLE schedule_fires ADD CONSTRAINT fixture_fail CHECK(outcome<>'created')",
    )
    .await
    .unwrap();
    run_batch(&mut c, &f.auth, start, at).await.unwrap();
    assert_eq!(cursor(&c, schedule).await, at);
    assert!(fires(&c, schedule).await.is_empty());
    assert_eq!(
        c.query_one("SELECT count(*) FROM tasks", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    c.batch_execute("ALTER TABLE schedule_fires DROP CONSTRAINT fixture_fail")
        .await
        .unwrap();
    run_batch(&mut c, &f.auth, start, at).await.unwrap();
    assert_eq!(fires(&c, schedule).await[0].1, "created");
    let later = at + Duration::minutes(1);
    let restored = f
        .schedule(&mut c, later, "* * * * *", "UTC", "skip", "skip")
        .await;
    let recovery = Uuid::new_v4();
    c.execute("INSERT INTO platform_recovery_state(recovery_id,backup_id,phase) VALUES($1,$2,'importing')",
        &[&recovery,&Uuid::new_v4()]).await.unwrap();
    run_batch(&mut c, &f.auth, start, later).await.unwrap();
    assert!(fires(&c, restored).await.is_empty());
    let tx = c.transaction().await.unwrap();
    crate::recovery::reset_rows(
        &tx,
        f.ctx.subject,
        "independent recovered schedule password",
        recovery,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    c.execute("UPDATE platform_recovery_state SET phase='ready',recovery_owner_id=$2 WHERE recovery_id=$1",
        &[&recovery,&f.ctx.subject]).await.unwrap();
    run_batch(&mut c, &f.auth, start, later).await.unwrap();
    assert_eq!(
        fires(&c, restored).await,
        [(later, "paused".to_owned(), None)]
    );
    assert_eq!(
        c.query_one("SELECT count(*) FROM tasks", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1,
    );
}

#[tokio::test]
async fn issuer_expiry_while_writing_steps_pauses_fire_without_child_delegation() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.db.connect().await.unwrap();
    let mut blocker = f.db.connect().await.unwrap();
    let observer = f.db.connect().await.unwrap();
    let at = time("2025-03-01T09:00:00Z");
    let schedule = f
        .schedule(&mut c, at, "* * * * *", "UTC", "skip", "skip")
        .await;
    let until: DateTime<Utc> = observer
        .query_one(
            "UPDATE execution_delegations e SET expires_at=clock_timestamp()+interval '6 seconds'
          FROM schedules s WHERE s.id=$1 AND e.id=s.delegation_id RETURNING e.expires_at",
            &[&schedule],
        )
        .await
        .unwrap()
        .get(0);
    let hold = blocker.transaction().await.unwrap();
    hold.batch_execute("LOCK TABLE task_steps IN SHARE MODE")
        .await
        .unwrap();
    let firing =
        tokio::spawn(
            async move { run_batch(&mut c, &f.auth, at - Duration::seconds(1), at).await },
        );
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            let blocked: bool = observer
                .query_one(
                    "SELECT EXISTS(SELECT FROM pg_locks
                WHERE relation='task_steps'::regclass AND mode='RowExclusiveLock' AND NOT granted)",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            assert!(!firing.is_finished());
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("fire must reach the blocked TaskStep insert");
    observer.query_one(
        "SELECT pg_sleep(greatest(extract(epoch FROM $1::timestamptz-clock_timestamp()),0)+0.05)",
        &[&until],
    ).await.unwrap();
    hold.rollback().await.unwrap();
    firing.await.unwrap().unwrap();
    assert_eq!(
        fires(&observer, schedule).await,
        [(at, "paused".to_owned(), None)]
    );
    let count = observer
        .query_one(
            "SELECT (SELECT count(*) FROM execution_delegations),
                                           (SELECT count(*) FROM tasks)",
            &[],
        )
        .await
        .unwrap();
    assert_eq!((count.get::<_, i64>(0), count.get::<_, i64>(1)), (1, 0));
}

#[tokio::test]
async fn loss_of_leadership_before_commit_rolls_back_fire_task_and_audit() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.db.connect().await.unwrap();
    let at = time("2025-03-01T09:00:00Z");
    let schedule = f
        .schedule(&mut c, at, "* * * * *", "UTC", "skip", "skip")
        .await;
    let checks = AtomicUsize::new(0);
    let stopping = || checks.fetch_add(1, Ordering::Relaxed) >= 1;
    let error = super::run_batch(&mut c, &f.auth, at - Duration::seconds(1), at, &stopping)
        .await
        .unwrap_err();
    assert_eq!(error.code, "NOT_READY");
    assert_eq!(cursor(&c, schedule).await, at);
    assert!(fires(&c, schedule).await.is_empty());
    let counts = c
        .query_one(
            "SELECT (SELECT count(*) FROM tasks),
        (SELECT count(*) FROM audit_events WHERE action='schedule.fire')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!((counts.get::<_, i64>(0), counts.get::<_, i64>(1)), (0, 0));
}

#[tokio::test]
async fn explicit_retry_waits_for_new_scheduled_task_and_checks_unknown_siblings() {
    use axum::http::HeaderMap;

    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.db.connect().await.unwrap();
    let mut held = f.db.connect().await.unwrap();
    let mut operator = f.db.connect().await.unwrap();
    let at = time("2025-03-01T09:00:00Z");
    let start = at - Duration::seconds(1);
    let schedule = f
        .schedule(&mut c, at, "* * * * *", "UTC", "skip", "queue_one")
        .await;
    run_batch(&mut c, &f.auth, start, at).await.unwrap();
    let task = fires(&c, schedule).await[0].2.unwrap();
    let worker = Uuid::new_v4();
    let attempt = crate::task_engine::claim(&f.db, &f.auth, worker)
        .await
        .unwrap()
        .unwrap();
    let step = crate::task_engine::commit(&f.db, &f.auth, worker, &json!({
        "attempt_id":attempt["id"],"execution_id":attempt["execution_id"],"fence":attempt["fence"],
        "state":"unknown","output":null,"error":null
    })).await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "if-match",
        format!("\"rev-{}\"", step["revision"]).parse().unwrap(),
    );
    let tx = c.transaction().await.unwrap();
    crate::db::auth_lock(&tx, false).await.unwrap();
    crate::tasks::execute(
        &tx,
        &f.auth,
        &f.ctx,
        "resolveUnknownStep",
        &[(
            "step_id".to_owned(),
            step["id"].as_str().unwrap().to_owned(),
        )]
        .into(),
        &json!({"resolution":"confirmed_not_executed","evidence":"provider confirms no execution"}),
        &headers,
        "",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let row = c
        .query_one("SELECT revision,state FROM tasks WHERE id=$1", &[&task])
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(1), "failed");
    headers.insert(
        "if-match",
        format!("\"rev-{}\"", row.get::<_, i64>(0)).parse().unwrap(),
    );
    let due = at + Duration::minutes(1);
    let fire = held.transaction().await.unwrap();
    assert!(crate::db::try_auth_lock_shared(&fire).await.unwrap());
    let row = fire
        .query_one(super::CANDIDATE, &[&due, &Vec::<Uuid>::new(), &true])
        .await
        .unwrap();
    super::reconcile(&fire, &f.auth, &row, schedule, start, due)
        .await
        .unwrap();

    let operator_pid: i32 = operator
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let auth = f.auth.clone();
    let ctx = f.ctx.clone();
    let retry_headers = headers.clone();
    let request = tokio::spawn(async move {
        let tx = operator.transaction().await.unwrap();
        crate::db::auth_lock(&tx, false).await.unwrap();
        let result = crate::tasks::retry::request(&tx, &auth, &ctx, task, &retry_headers).await;
        tx.rollback().await.unwrap();
        result
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let blocked: bool = c
                .query_one(
                    "SELECT cardinality(pg_blocking_pids($1))>0",
                    &[&operator_pid],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            assert!(!request.is_finished());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("retry must wait on the schedule row before locking its Task");
    fire.commit().await.unwrap();
    let rejected = request.await.unwrap().unwrap_err();
    assert_eq!(
        rejected.message,
        "Another run of the schedule is still active"
    );
    let sibling = fires(&c, schedule).await[1].2.unwrap();
    c.execute("UPDATE tasks SET state='unknown' WHERE id=$1", &[&sibling])
        .await
        .unwrap();
    let tx = c.transaction().await.unwrap();
    crate::db::auth_lock(&tx, false).await.unwrap();
    assert_eq!(
        crate::tasks::retry::request(&tx, &f.auth, &f.ctx, task, &headers)
            .await
            .unwrap_err()
            .message,
        "Another run of the schedule is still active"
    );
    tx.rollback().await.unwrap();

    c.execute(
        "UPDATE tasks SET state='succeeded' WHERE id=$1",
        &[&sibling],
    )
    .await
    .unwrap();
    let tx = c.transaction().await.unwrap();
    crate::db::auth_lock(&tx, false).await.unwrap();
    let (retried, _) = crate::tasks::retry::request(&tx, &f.auth, &f.ctx, task, &headers)
        .await
        .unwrap();
    assert_eq!(retried["state"], "queued");
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn runner_waits_for_readiness_then_processes_startup_and_stops() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.db.connect().await.unwrap();
    let missed = time("2025-03-01T09:00:00Z");
    let schedule = f
        .schedule(&mut c, missed, "* * * * *", "UTC", "run_once", "skip")
        .await;
    let app = f.app(false).await;
    let runner = tokio::spawn(crate::run_schedules(app.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(fires(&c, schedule).await.is_empty());
    app.ready.store(true, Ordering::SeqCst);
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            let ledger = fires(&c, schedule).await;
            if !ledger.is_empty() {
                assert_eq!(ledger.len(), 1);
                assert_eq!(ledger[0].1, "created");
                assert!(ledger[0].0 > missed);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("the ready runner must process the recorded missed fire");
    app.shutdown.store(true, Ordering::SeqCst);
    tokio::time::timeout(std::time::Duration::from_secs(2), runner)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn runner_fires_younger_schedule_while_older_workflow_is_locked() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.db.connect().await.unwrap();
    let older_workflow = f.workflow;
    let older_due = time("2025-03-01T09:00:00Z");
    let older = f
        .schedule(&mut c, older_due, "* * * * *", "UTC", "run_once", "skip")
        .await;
    let younger_workflow = Uuid::new_v4();
    c.execute(
        "INSERT INTO workflows(id,owner_id,name,version,spec,status,created_at,updated_at)
         SELECT $2,owner_id,name,version+1,spec,status,now(),now() FROM workflows WHERE id=$1",
        &[&older_workflow, &younger_workflow],
    )
    .await
    .unwrap();
    let f = Fixture {
        workflow: younger_workflow,
        ..f
    };
    let younger = f
        .schedule(
            &mut c,
            older_due + Duration::minutes(1),
            "* * * * *",
            "UTC",
            "run_once",
            "skip",
        )
        .await;

    let mut locker = f.db.connect().await.unwrap();
    let hold = locker.transaction().await.unwrap();
    hold.query_one(
        "SELECT id FROM workflows WHERE id=$1 FOR UPDATE",
        &[&older_workflow],
    )
    .await
    .unwrap();
    let app = f.app(true).await;
    let runner = tokio::spawn(crate::run_schedules(app.clone()));
    let task = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            if let Some((_, outcome, task)) = fires(&c, younger).await.into_iter().next() {
                assert_eq!(outcome, "created");
                break task.unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("the younger schedule must fire without waiting for the older workflow lock");
    assert_eq!(
        c.query_one("SELECT workflow_id FROM tasks WHERE id=$1", &[&task])
            .await
            .unwrap()
            .get::<_, Uuid>(0),
        younger_workflow
    );
    assert!(fires(&c, older).await.is_empty());
    assert_eq!(cursor(&c, older).await, older_due);

    hold.rollback().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            if let Some((_, outcome, task)) = fires(&c, older).await.into_iter().next() {
                assert_eq!(outcome, "created");
                assert!(task.is_some());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("the older schedule must remain eligible after the workflow lock is released");
    app.shutdown.store(true, Ordering::SeqCst);
    tokio::time::timeout(std::time::Duration::from_secs(2), runner)
        .await
        .unwrap()
        .unwrap();
}
