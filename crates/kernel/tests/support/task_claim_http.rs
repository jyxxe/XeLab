//! Claims against the fixed operation from the signed HTTP echo example.
use super::{request, Fixture};
use axum::http::Method;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};
use tokio_postgres::Client;
use uuid::Uuid;
use xelab_kernel::{artifact, contracts, http_contracts, task_engine::claim};

async fn register_http_echo(f: &Fixture, db: &Client) -> Uuid {
    let publisher = db.query_one(
        "SELECT p.id,p.public_key_pem FROM publishers p JOIN installations n ON n.publisher_id=p.id
         WHERE n.id=$1",
        &[&f.installation],
    ).await.unwrap();
    let publisher_id: Uuid = publisher.get("id");
    let dir = tempfile::tempdir().unwrap();
    let built = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/http/build_artifact.py"))
        .arg("--publisher-id")
        .arg(publisher_id.to_string())
        .arg("--output")
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let raw = fs::read(dir.path().join("xelab/plugin.json")).unwrap();
    let manifest = contracts::strict_json(&raw, 1_048_576).unwrap();
    let openapi = fs::read(dir.path().join("xelab/http/openapi.json")).unwrap();
    artifact::validate_profile(&manifest).unwrap();

    // Sign the reference manifest with the workflow fixture's trusted publisher.
    let image = contracts::digest(b"HTTP claim fixture image");
    let envelope = artifact::signature_envelope(&manifest, &image, &raw).unwrap();
    let signature = SigningKey::from_bytes(&[13; 32]).sign(envelope.as_bytes());
    artifact::verify_signature(
        publisher.get("public_key_pem"),
        &STANDARD.encode(signature.to_bytes()),
        &manifest,
        &image,
        &raw,
    )
    .unwrap();
    let installation = Uuid::new_v4();
    db.execute(
        "INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,
                                   manifest,state,created_at,updated_at)
         VALUES($1,$2,$3,$4,$5,$6,$7,'installed',now(),now())",
        &[
            &installation,
            &manifest["plugin_id"].as_str().unwrap(),
            &manifest["version"].as_str().unwrap(),
            &publisher_id,
            &image,
            &contracts::digest(&raw),
            &manifest,
        ],
    )
    .await
    .unwrap();
    http_contracts::store(db, installation, &manifest, Some(&openapi))
        .await
        .unwrap();
    let spec = json!({"installation_id":installation, "enabled":true,
                      "limits":manifest["minimum_resources"]});
    db.execute(
        "INSERT INTO deployments(id,owner_id,installation_id,spec,approval,phase,created_at,updated_at)
         VALUES($1,$2,$3,$4,'{\"rules\":[],\"egress\":[]}','waiting_resources',now(),now())",
        &[&Uuid::new_v4(), &f.owner, &installation, &spec],
    ).await.unwrap();
    let definition = manifest["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["target"]["http_operation_id"] == "echoMessage")
        .unwrap();
    let skill = request(
        &f.router,
        Method::POST,
        "/api/platform/v1/skills",
        Some(&f.token),
        Some(&json!({"installation_id":installation, "definition":definition, "enabled":true})),
        &[("idempotency-key", "register-http-echo")],
    )
    .await
    .expect(201);
    contracts::id(&skill.body, "id").unwrap()
}

async fn submit(f: &Fixture, db: &Client, workflow: Uuid, key: &str) -> Uuid {
    let task = request(
        &f.router,
        Method::POST,
        "/api/platform/v1/tasks",
        Some(&f.token),
        Some(&json!({"workflow_id":workflow, "input":{}})),
        &[("idempotency-key", key)],
    )
    .await
    .expect(201);
    let task = contracts::id(&task.body, "id").unwrap();
    let row = db
        .query_one(
            "SELECT s.id,t.admission->'steps'->0->'binding' AS binding
         FROM task_steps s JOIN tasks t ON t.id=s.task_id WHERE s.task_id=$1 AND s.state='ready'",
            &[&task],
        )
        .await
        .unwrap();
    let binding: Value = row.get("binding");
    assert_eq!(binding["kind"], "skill_http");
    assert_eq!(binding["operation_id"], "echoMessage");
    assert_eq!(binding["method"], "POST");
    assert_eq!(binding["path"], "/plugins/org.xelab.http-echo/echo");
    row.get("id")
}

async fn assert_paused_without_attempt(db: &Client, step: Uuid) {
    let row = db.query_one(
        "SELECT t.state,s.fence,s.attempt_count,(SELECT count(*) FROM attempts WHERE step_id=s.id)
         AS attempts FROM tasks t JOIN task_steps s ON s.task_id=t.id WHERE s.id=$1",
        &[&step],
    ).await.unwrap();
    assert_eq!(row.get::<_, String>("state"), "paused_authorization");
    assert_eq!(row.get::<_, i64>("fence"), 0);
    assert_eq!(row.get::<_, i32>("attempt_count"), 0);
    assert_eq!(row.get::<_, i64>("attempts"), 0);
}

#[tokio::test]
async fn fixed_http_skill_claim_requires_the_original_route_grant() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let skill = register_http_echo(&f, &db).await;
    let mut spec = f.spec("HTTP claim");
    spec["steps"][0].as_object_mut().unwrap().remove("contract");
    spec["steps"][0]["skill_id"] = json!(skill);
    let workflow = f
        .publish(&f.token, &spec, "publish-http-echo")
        .await
        .expect(201);
    let workflow = contracts::id(&workflow.body, "id").unwrap();
    let damaged = submit(&f, &db, workflow, "damaged-definition-digest").await;
    let wrong_digest = contracts::digest(b"different Skill definition");
    assert_eq!(
        db.execute(
            "UPDATE tasks t SET admission=jsonb_set(t.admission,
                 '{steps,0,binding,definition_digest}',to_jsonb($2::text),false)
             FROM task_steps s WHERE s.id=$1 AND t.id=s.task_id
               AND t.admission #>> '{steps,0,binding,definition_digest}' <> $2",
            &[&damaged, &wrong_digest],
        )
        .await
        .unwrap(),
        1
    );
    let first = submit(&f, &db, workflow, "original-grant").await;
    let attempt = claim(&f.db, &f.auth, Uuid::new_v4())
        .await
        .unwrap()
        .expect("HTTP step lease");
    assert_eq!(contracts::id(&attempt, "step_id").unwrap(), first);
    assert_eq!(attempt["state"], "leased");
    assert_paused_without_attempt(&db, damaged).await;

    let second = submit(&f, &db, workflow, "revoked-grant").await;
    assert_eq!(
        db.execute(
            "UPDATE grants SET status='revoked'
         WHERE subject_id=$1 AND scope->>'kind'='plugin' AND status='active'",
            &[&f.owner],
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(claim(&f.db, &f.auth, Uuid::new_v4()).await.unwrap(), None);
    assert_paused_without_attempt(&db, second).await;
}
