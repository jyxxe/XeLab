//! Real PostgreSQL clock/row-lock regressions for candidate delivery. These
//! direct backend calls deliberately outlive the public metadata RPC budget;
//! they do not change or claim acceptance of the unchanged two-second wrapper.
//! Workload and provider metadata remain component fixtures, not OCI evidence.
use super::*;
use chrono::{DateTime, Utc};
use tokio_postgres::{Client, Transaction};

struct Catalog {
    actor: Uuid,
    instance: Uuid,
    skill: Uuid,
    installation: Uuid,
}

impl Catalog {
    fn workload(&self) -> Workload {
        Workload {
            instance: self.instance,
            generation: 7,
        }
    }
}

async fn catalog(f: &Fixture) -> Catalog {
    let (actor, instance) = f.workload().await;
    proxy(f, actor, proxy_rules()).await;
    let (provider, _) = f.workload().await;
    let (skill, _) = skill(f, provider).await;
    let installation = f
        .app
        .db
        .connect()
        .await
        .unwrap()
        .query_one(
            "SELECT installation_id FROM deployments WHERE id=$1",
            &[&provider],
        )
        .await
        .unwrap()
        .get(0);
    Catalog {
        actor,
        instance,
        skill,
        installation,
    }
}

fn request(mut context: rpc::RpcContext) -> rpc::ListSkillCandidatesRequest {
    context.deadline_unix_ms = (Utc::now() + chrono::Duration::seconds(60)).timestamp_millis();
    rpc::ListSkillCandidatesRequest {
        context: Some(context),
        plugin_ids: vec!["org.xelab.echo-python".into()],
        capability_names: vec!["org.xelab.echo".into()],
        page_size: 1,
        cursor: String::new(),
    }
}

async fn lock_candidate(c: &mut Client, installation: Uuid) -> (Transaction<'_>, i32) {
    let pid = c
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let tx = c.transaction().await.unwrap();
    tx.query_one(
        "SELECT id FROM installations WHERE id=$1 FOR UPDATE",
        &[&installation],
    )
    .await
    .unwrap();
    (tx, pid)
}

async fn wait_candidate_block(c: &Client, blocker: i32) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row = c.query_opt("SELECT pid,query FROM pg_stat_activity WHERE datname=current_database() AND state='active' AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE 'SELECT n.manifest FROM installations n JOIN publishers p%' AND query LIKE '%FOR SHARE OF n,p'",&[&blocker]).await.unwrap();
            if let Some(row) = row {
                let waiter: i32 = row.get("pid");
                assert_ne!(waiter, blocker);
                assert!(row.get::<_, String>("query").contains("WHERE n.id=$1"));
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("initial actor authority must finish before the selected provider's installed_candidate row wait");
}

async fn wait_clock(c: &Client, expiry: DateTime<Utc>) {
    assert!(
        c.query_one("SELECT clock_timestamp() < $1", &[&expiry])
            .await
            .unwrap()
            .get::<_, bool>(0),
        "fixture must reach the actual post-authority candidate wait before expiry"
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if c.query_one("SELECT clock_timestamp() > $1", &[&expiry])
                .await
                .unwrap()
                .get::<_, bool>(0)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("actual PostgreSQL clock must cross the prepared expiry");
}

async fn assert_running(c: &Client, call: Uuid, unexpired: bool) {
    let row = c
        .query_one(
            "SELECT state,deadline_at > clock_timestamp() AS unexpired FROM calls WHERE id=$1",
            &[&call],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>("state"), "running");
    assert_eq!(row.get::<_, bool>("unexpired"), unexpired);
}

#[tokio::test]
async fn host_candidates_recheck_parent_expiry_after_catalog_row_wait() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let catalog = catalog(&f).await;
    let source = f.source(b"candidate source".to_vec()).await;
    let (call, context) = f.delegation(catalog.actor, catalog.instance, source).await;
    let observer = f.app.db.connect().await.unwrap();
    let expiry = observer.query_one("UPDATE calls SET deadline_at=clock_timestamp()+interval '15 seconds' WHERE id=$1 RETURNING deadline_at",&[&call]).await.unwrap().get(0);
    let mut blocker = f.app.db.connect().await.unwrap();
    let (lock, pid) = lock_candidate(&mut blocker, catalog.installation).await;
    let app = f.app.clone();
    let workload = catalog.workload();
    let task = tokio::spawn(async move {
        host_calls::list_skill_candidates(&app, workload, request(context)).await
    });
    wait_candidate_block(&observer, pid).await;
    assert!(!task.is_finished());
    wait_clock(&observer, expiry).await;
    assert_running(&observer, call, false).await;
    assert!(!task.is_finished());
    lock.commit().await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("candidate request must finish after provider row release")
        .unwrap()
        .expect_err("a completed initial proof cannot release candidates after its parent expires");
    assert_eq!(error.code, "DEADLINE_EXCEEDED");
}

async fn external_source(f: &Fixture, skill: Uuid, credential: bool) -> (Uuid, Option<Uuid>, Uuid) {
    let mut c = f.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    crate::db::auth_lock(&tx, true).await.unwrap();
    let caller = f
        .app
        .auth
        .authenticate(&tx, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let issuer = Uuid::new_v4();
    tx.execute("INSERT INTO accounts(id,kind,status,display_name,created_at,updated_at) VALUES($1,'service','active','candidate Skill source issuer',now(),now())",&[&issuer]).await.unwrap();
    tx.execute(
        "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
        &[&issuer],
    )
    .await
    .unwrap();
    let scope = json!({"kind":"skill","match":"ids","ids":[skill]});
    tx.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$3,$4,'Owner',$5,'active',now(),now())",&[&Uuid::new_v4(),&issuer,&caller.subject,&caller.credential,&scope]).await.unwrap();
    let credential = if credential {
        let id = Uuid::new_v4();
        let (_, hash) = f.app.auth.random_token();
        tx.execute("INSERT INTO credentials(id,owner_id,kind,hmac_key_id,verification_digest,status,conditions,expires_at,created_at,updated_at) VALUES($1,$2,'session',$3,$4,'active','{}',clock_timestamp()+interval '1 hour',now(),now())",&[&id,&issuer,&f.app.auth.key_id.as_str(),&hash]).await.unwrap();
        Some(id)
    } else {
        None
    };
    let delegated = Uuid::new_v4();
    tx.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$3,$4,'Owner',$5,'active',now(),now())",&[&delegated,&caller.subject,&issuer,&credential,&scope]).await.unwrap();
    tx.commit().await.unwrap();
    (issuer, credential, delegated)
}

async fn freeze_external_read(
    f: &Fixture,
    call: Uuid,
    skill: Uuid,
    issuer: Uuid,
    credential: Option<Uuid>,
    delegated: Uuid,
) -> DateTime<Utc> {
    let mut c = f.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    crate::db::auth_lock(&tx, true).await.unwrap();
    let caller = f
        .app
        .auth
        .authenticate(&tx, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let actual = crate::policy::Policy::for_context(&tx, &caller)
        .credential(caller.credential, 0)
        .await
        .unwrap();
    let resource = auth::Resource::new("skill", skill, Some(caller.subject));
    let saved = crate::host_blobs::saved(
        tx.query_one(
            "SELECT proxy_source_paths FROM host_rpc_contexts WHERE call_id=$1",
            &[&call],
        )
        .await
        .unwrap()
        .get(0),
    )
    .unwrap();
    let original: Vec<_> = saved
        .iter()
        .filter(|path| path.rule["action"] == "skill.read" && path.sources.contains(&delegated))
        .cloned()
        .collect();
    assert!(
        !original.is_empty() && original.iter().all(|path| actual.contains(path)),
        "every frozen external Skill path must come from original admission and current real Policy"
    );
    assert!(auth::rules_allow(
        &crate::policy::rules(&original),
        "skill.read",
        &resource,
        caller.peer
    ));
    let independent: Vec<_> = actual
        .iter()
        .filter(|path| path.rule["action"] == "skill.read" && !path.sources.contains(&delegated))
        .cloned()
        .collect();
    assert!(auth::rules_allow(
        &crate::policy::rules(&independent),
        "skill.read",
        &resource,
        caller.peer
    ));
    let mut narrowed: Vec<_> = saved
        .into_iter()
        .filter(|path| path.rule["action"] != "skill.read")
        .collect();
    narrowed.extend(original);
    tx.execute(
        "UPDATE host_rpc_contexts SET proxy_source_paths=$2 WHERE call_id=$1",
        &[&call, &json!(narrowed)],
    )
    .await
    .unwrap();
    // All source/clock setup precedes the backend's shared authorization fence.
    // Only real clock passage and the unrelated provider row lock follow.
    let expiry = if let Some(credential) = credential {
        tx.query_one("UPDATE credentials SET expires_at=clock_timestamp()+interval '15 seconds' WHERE id=$1 RETURNING expires_at",&[&credential]).await.unwrap().get(0)
    } else {
        tx.query_one("UPDATE accounts SET expires_at=clock_timestamp()+interval '15 seconds' WHERE id=$1 RETURNING expires_at",&[&issuer]).await.unwrap().get(0)
    };
    tx.commit().await.unwrap();
    expiry
}

#[tokio::test]
async fn host_candidates_recheck_external_skill_source_expiry_after_catalog_row_wait() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let catalog = catalog(&f).await;
    let source = f.source(b"candidate source".to_vec()).await;
    for credential_expiry in [false, true] {
        let (issuer, credential, delegated) =
            external_source(&f, catalog.skill, credential_expiry).await;
        let (call, context) = f.delegation(catalog.actor, catalog.instance, source).await;
        let expiry =
            freeze_external_read(&f, call, catalog.skill, issuer, credential, delegated).await;
        let observer = f.app.db.connect().await.unwrap();
        let mut blocker = f.app.db.connect().await.unwrap();
        let (lock, pid) = lock_candidate(&mut blocker, catalog.installation).await;
        let app = f.app.clone();
        let workload = catalog.workload();
        let task = tokio::spawn(async move {
            host_calls::list_skill_candidates(&app, workload, request(context)).await
        });
        wait_candidate_block(&observer, pid).await;
        assert!(!task.is_finished());
        wait_clock(&observer, expiry).await;
        assert_running(&observer, call, true).await;
        assert!(!task.is_finished());
        lock.commit().await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("candidate request must finish after provider row release")
            .unwrap()
            .expect_err("expired frozen Skill sources cannot borrow a caller's independent Owner alternative");
        assert_eq!(error.code, "PERMISSION_DENIED");
        assert_running(&observer, call, true).await;
        let caller = f
            .app
            .auth
            .authenticate(
                &observer,
                &f.token,
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        auth::authorize(
            &observer,
            &caller,
            "skill.read",
            &auth::Resource::new("skill", catalog.skill, Some(caller.subject)),
        )
        .await
        .expect("the caller's independent original Owner permission remains valid");
    }
}

#[tokio::test]
async fn host_candidates_preserve_nonexpired_authority_after_catalog_row_wait() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let catalog = catalog(&f).await;
    let source = f.source(b"candidate source".to_vec()).await;
    let (call, context) = f.delegation(catalog.actor, catalog.instance, source).await;
    let observer = f.app.db.connect().await.unwrap();
    let mut blocker = f.app.db.connect().await.unwrap();
    let (lock, pid) = lock_candidate(&mut blocker, catalog.installation).await;
    let app = f.app.clone();
    let workload = catalog.workload();
    let task = tokio::spawn(async move {
        host_calls::list_skill_candidates(&app, workload, request(context)).await
    });
    wait_candidate_block(&observer, pid).await;
    assert!(!task.is_finished());
    assert_running(&observer, call, true).await;
    lock.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("candidate request must finish after provider row release")
        .unwrap()
        .unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].skill_id, catalog.skill.to_string());
    assert!(result.items[0].ready);
    assert_running(&observer, call, true).await;
}
