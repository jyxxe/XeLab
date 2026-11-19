//! A large admitted proof and the following task must both be claimable.
use crate::{auth, backups::tests::Fixture, contracts, db, identity, tasks, workflows};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use tokio_postgres::Client;
use uuid::Uuid;

async fn submit(c: &mut Client, ctx: &auth::Context, workflow: Uuid) -> Uuid {
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, false).await.unwrap();
    let task = tasks::create(&tx, ctx, &json!({"workflow_id":workflow,"input":{}}))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    contracts::id(&task, "id").unwrap()
}

#[tokio::test]
async fn claim_91_independent_grants_then_the_next_healthy_task() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.app.db.connect().await.unwrap();
    let auth = &f.app.auth;
    let peer = "127.0.0.1".parse().unwrap();
    let owner = auth
        .authenticate(&c, &f.token, peer, Uuid::new_v4())
        .await
        .unwrap();
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/secret-manifest.json")).unwrap();
    let publisher: Uuid = c
        .query_one("SELECT id FROM publishers", &[])
        .await
        .unwrap()
        .get(0);
    manifest["publisher_id"] = json!(publisher);
    let cap = &manifest["capabilities"][0];
    let contract = &cap["contract"];
    let digest = contracts::digest(&serde_json::to_vec(&manifest).unwrap());
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at)
               VALUES($1,$2,'1.0.0',$3,$4,$4,$5,'installed',now(),now())",
        &[&Uuid::new_v4(), &manifest["plugin_id"].as_str().unwrap(), &publisher, &digest,&manifest]).await.unwrap();
    c.execute(
        "INSERT INTO capability_contracts(name,version,schema_digest,spec) VALUES($1,$2,$3,$4)",
        &[
            &contract["name"].as_str().unwrap(),
            &contract["version"].as_str().unwrap(),
            &contract["schema_digest"].as_str().unwrap(),
            cap,
        ],
    )
    .await
    .unwrap();
    let spec = json!({"name":"many-grants","input_schema":{"type":"object"},"steps":[{
        "id":"echo","contract":contract,"depends_on":[],"input_mapping":{"message":{"literal":"hello"}},"timeout_ms":30000}]});
    let tx = c.transaction().await.unwrap();
    let (workflow, _) = workflows::execute(
        &tx,
        auth,
        &owner,
        "createWorkflow",
        &Default::default(),
        &spec,
        &Default::default(),
        "",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let workflow = contracts::id(&workflow, "id").unwrap();

    // A standalone grantee, as in the workflow claim fixtures, has no inherited
    // capability. Issue the first grant normally and batch the identical rows.
    let subject = Uuid::new_v4();
    c.execute("INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at)
               SELECT $1,'user','active','Task submitter',password_hash,now(),now() FROM accounts WHERE id=$2",
        &[&subject,&owner.subject]).await.unwrap();
    c.execute("INSERT INTO account_closure VALUES($1,$1,0)", &[&subject])
        .await
        .unwrap();
    c.execute(
        "INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at)
               SELECT $1,spec,current_date,now(),now() FROM quotas WHERE account_id=$2",
        &[&subject, &owner.subject],
    )
    .await
    .unwrap();
    let params = HashMap::from([("account_id".into(), subject.to_string())]);
    for (role, scope) in [
        (
            "Owner",
            json!({"kind":"workflow","match":"ids","ids":[workflow]}),
        ),
        (
            "Member",
            json!({"kind":"task","match":"owned_by","owner_id":subject}),
        ),
        ("Guest", json!({"kind":"capability","match":"all"})),
    ] {
        let body = json!({"source":"role","role_id":role,"scope":scope});
        identity::authorize(&c, &owner, "createGrant", &params, &body)
            .await
            .unwrap();
        identity::execute(
            &c,
            auth,
            &owner,
            "createGrant",
            &params,
            &body,
            &Default::default(),
            "",
        )
        .await
        .unwrap();
    }
    let added = c.execute(
        "INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at)
         SELECT gen_random_uuid(),subject_id,issuer_id,issuer_credential_id,role_id,scope,status,now(),now()
         FROM grants CROSS JOIN generate_series(1,90) WHERE subject_id=$1 AND role_id='Guest'",
         &[&subject],
    ).await.unwrap();
    assert_eq!(added, 90);
    let login = auth
        .login(
            &c,
            subject,
            "correct horse backup fixture password".into(),
            peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let grantee = auth
        .authenticate(
            &c,
            login["access_token"].as_str().unwrap(),
            peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let large = submit(&mut c, &grantee, workflow).await;
    let next = submit(&mut c, &owner, workflow).await;
    let paths: Value = c
        .query_one(
            "SELECT admission #> '{steps,0,targets,0,paths}' FROM tasks WHERE id=$1",
            &[&large],
        )
        .await
        .unwrap()
        .get(0);
    let paths: Vec<crate::policy::Path> = serde_json::from_value(paths).unwrap();
    assert_eq!(paths.len(), 91);
    assert!(paths.iter().all(|p| p.rule == paths[0].rule));
    let mut unions = BTreeSet::new();
    for (i, left) in paths.iter().enumerate() {
        for right in &paths[i..] {
            unions.insert(
                left.sources
                    .union(&right.sources)
                    .copied()
                    .collect::<BTreeSet<_>>(),
            );
        }
    }
    assert_eq!(unions.len(), 4186);
    for task in [large, next] {
        let expected: Uuid = c
            .query_one("SELECT id FROM task_steps WHERE task_id=$1", &[&task])
            .await
            .unwrap()
            .get(0);
        let lease = tasks::task_engine::claim(&f.app.db, auth, Uuid::new_v4())
            .await
            .unwrap()
            .expect("healthy task must be leased");
        assert_eq!(contracts::id(&lease, "step_id").unwrap(), expected);
    }
}
