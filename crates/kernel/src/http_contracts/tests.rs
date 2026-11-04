//! Real PostgreSQL metadata tests. Installation rows are component fixtures;
//! signed OCI admission and live gateway execution have separate acceptance.
use super::*;
use crate::{auth, db::Db};
use std::{fs, path::Path, process::Command};
use tokio_postgres::Client;

pub(crate) fn fixture_manifest(publisher: Uuid) -> (tempfile::TempDir, Value, Vec<u8>) {
    let directory = tempfile::tempdir().unwrap();
    let builder =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/http/build_artifact.py");
    let output = Command::new("python3")
        .arg(builder)
        .arg("--publisher-id")
        .arg(publisher.to_string())
        .arg("--output")
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "reference artifact builder failed");
    let manifest = crate::contracts::strict_json(
        &fs::read(directory.path().join("xelab/plugin.json")).unwrap(),
        1048576,
    )
    .unwrap();
    let raw = fs::read(directory.path().join("xelab/http/openapi.json")).unwrap();
    (directory, manifest, raw)
}

async fn fresh_database() -> Option<(Db, Client)> {
    let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: immutable HTTP metadata tests require PostgreSQL");
        return None;
    };
    let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
    let name = format!("http_contract_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let db = Db::new(format!("{base} dbname={name}")).unwrap();
    let c = db.connect().await.unwrap();
    Some((db, c))
}

struct Fixture {
    db: Db,
    c: Client,
    owner: Uuid,
    installation: Uuid,
    manifest: Value,
    raw: Vec<u8>,
    directory: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Option<Self> {
        let (db, c) = fresh_database().await?;
        db.migrate().await.unwrap();
        db.migrate().await.unwrap();
        let owner=auth::bootstrap(&c,"HTTP metadata owner","metadata component owner password",&json!({"maximum_depth":4,"maximum_descendants":20,"maximum_api_keys":50,"maximum_concurrent_calls":16,"daily_calls":10000})).await.unwrap();
        let publisher = Uuid::new_v4();
        let (directory, manifest, raw) = fixture_manifest(publisher);
        let installation = Uuid::new_v4();
        c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'metadata-only HTTP publisher',$2,'fixture-only','trusted',now(),now())",&[&publisher,&format!("http-{publisher}")]).await.unwrap();
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'installed',now(),now())",&[&installation,&string(&manifest,"plugin_id").unwrap(),&string(&manifest,"version").unwrap(),&publisher,&digest(b"component image"),&digest(&fs::read(directory.path().join("xelab/plugin.json")).unwrap()),&manifest]).await.unwrap();
        Some(Self {
            db,
            c,
            owner,
            installation,
            manifest,
            raw,
            directory,
        })
    }
}

#[tokio::test]
async fn actual_postgres_v14_appends_http_schema_without_rewriting_checksums() {
    let Some((db, c)) = fresh_database().await else {
        return;
    };
    let mut steps = vec![crate::db::BASELINE.to_owned()];
    let mut files = fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect::<Vec<_>>();
    files.sort();
    for file in files.into_iter().take(13) {
        steps.push(fs::read_to_string(file).unwrap());
    }
    assert_eq!(steps.len(), 14);
    for (offset, sql) in steps.iter().enumerate() {
        c.batch_execute(sql).await.unwrap();
        c.execute(
            "INSERT INTO schema_migrations(version,checksum,applied_at) VALUES($1,$2,now())",
            &[&((offset + 1) as i64), &digest(sql.as_bytes())],
        )
        .await
        .unwrap();
    }
    assert!(db.check().await.is_err());
    db.migrate().await.unwrap();
    db.migrate().await.unwrap();
    db.check().await.unwrap();
    let rows = c
        .query(
            "SELECT version,checksum FROM schema_migrations ORDER BY version",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), crate::db::SCHEMA_VERSION as usize);
    for (offset, sql) in steps.iter().enumerate() {
        assert_eq!(
            rows[offset].get::<_, String>("checksum"),
            digest(sql.as_bytes())
        );
    }
    assert!(crate::db::snapshot_tables().contains(&"plugin_http_contracts"));
    assert!(crate::db::snapshot_tables().contains(&"plugin_http_approvals"));
    assert!(crate::db::snapshot_tables().contains(&"plugin_http_requests"));
}

#[tokio::test]
async fn actual_postgres_contract_bytes_are_immutable_and_match_the_original_member() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    store(&f.c, f.installation, &f.manifest, Some(&f.raw))
        .await
        .unwrap();
    store(&f.c, f.installation, &f.manifest, Some(&f.raw))
        .await
        .unwrap();
    assert!(!load(&f.c, f.installation)
        .await
        .unwrap()
        .operations
        .is_empty());
    assert_eq!(
        verify_image(&f.c, f.installation, &f.manifest, f.directory.path())
            .await
            .unwrap(),
        Some(f.raw.clone())
    );
    assert!(f
        .c
        .execute(
            "UPDATE plugin_http_contracts SET raw=$2 WHERE installation_id=$1",
            &[&f.installation, &b"{}".as_slice()]
        )
        .await
        .is_err());
    assert!(f
        .c
        .execute(
            "DELETE FROM plugin_http_contracts WHERE installation_id=$1",
            &[&f.installation]
        )
        .await
        .is_err());
    let mut altered = f.raw.clone();
    altered.push(b' ');
    fs::write(f.directory.path().join("xelab/http/openapi.json"), altered).unwrap();
    assert!(
        verify_image(&f.c, f.installation, &f.manifest, f.directory.path())
            .await
            .is_err()
    );
    f.c.execute("UPDATE publishers SET status='revoked'", &[])
        .await
        .unwrap();
    f.c.execute(
        "UPDATE installations SET state='uninstalled' WHERE id=$1",
        &[&f.installation],
    )
    .await
    .unwrap();
    validate_snapshot_bindings(&f.c).await.unwrap();
    f.db.check().await.unwrap();
}

#[tokio::test]
async fn actual_postgres_route_action_definition_is_current_and_scoped_to_plugins() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    store(&f.c, f.installation, &f.manifest, Some(&f.raw))
        .await
        .unwrap();
    let _compiled = load(&f.c, f.installation).await.unwrap();
    validate_actions(&f.c, &f.manifest).await.unwrap();
    f.c.execute("UPDATE permission_definitions SET resource_kinds='[\"account\"]' WHERE action='plugin.http.invoke'",&[]).await.unwrap();
    assert!(validate_actions(&f.c, &f.manifest).await.is_err());
    assert!(store(&f.c, f.installation, &f.manifest, Some(&f.raw))
        .await
        .is_err());
    f.c.execute(
        "DELETE FROM permission_definitions WHERE action='plugin.http.invoke'",
        &[],
    )
    .await
    .unwrap();
    assert!(validate_actions(&f.c, &f.manifest).await.is_err());
}

#[tokio::test]
async fn actual_postgres_registry_cache_reuses_only_verified_immutable_definitions() {
    let Some(mut f) = Fixture::new().await else {
        return;
    };
    store(&f.c, f.installation, &f.manifest, Some(&f.raw))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let first = load(&f.c, f.installation).await.unwrap();
    let cold = started.elapsed();
    let started = std::time::Instant::now();
    let second = load(&f.c, f.installation).await.unwrap();
    let hot = started.elapsed();
    assert!(
        Arc::ptr_eq(&first, &second),
        "the resident immutable registry must be reused"
    );
    eprintln!("HTTP registry component cold={cold:?} hot={hot:?}");
    assert!(registry_cache().lock().await.len() <= MAXIMUM_CACHED_REGISTRIES);
    // Inject a storage fault only inside a rolled-back fixture transaction.
    // Production immutability remains enabled; hot cache hits must still verify
    // the actual current bytes rather than silently serving a previous record.
    let tx = f.c.transaction().await.unwrap();
    tx.batch_execute(
        "ALTER TABLE plugin_http_contracts DISABLE TRIGGER plugin_http_contract_immutable",
    )
    .await
    .unwrap();
    tx.execute(
        "UPDATE plugin_http_contracts SET raw=$2 WHERE installation_id=$1",
        &[&f.installation, &b"{}".as_slice()],
    )
    .await
    .unwrap();
    tx.batch_execute(
        "ALTER TABLE plugin_http_contracts ENABLE TRIGGER plugin_http_contract_immutable",
    )
    .await
    .unwrap();
    assert!(load(&tx, f.installation).await.is_err());
    tx.rollback().await.unwrap();
    let mut altered = f.manifest.clone();
    altered["routes"][0]["http_contract_digest"] = json!(digest(b"wrong signed route digest"));
    let tx = f.c.transaction().await.unwrap();
    tx.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&f.installation, &altered],
    )
    .await
    .unwrap();
    assert!(load(&tx, f.installation).await.is_err());
    tx.rollback().await.unwrap();
    let mut changed = f.manifest.clone();
    changed["routes"][0]["timeout_ms"] =
        json!(unsigned(&changed["routes"][0]["timeout_ms"]).unwrap() + 1);
    f.c.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&f.installation, &changed],
    )
    .await
    .unwrap();
    let changed_registry = load(&f.c, f.installation).await.unwrap();
    assert!(
        !Arc::ptr_eq(&first, &changed_registry),
        "current Manifest content is part of the cache key"
    );
    assert!(registry_cache().lock().await.len() <= MAXIMUM_CACHED_REGISTRIES);
}

#[tokio::test]
async fn actual_postgres_corrupt_raw_contract_is_rejected_before_restore_or_routing() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    f.c.execute(
        "INSERT INTO plugin_http_contracts(installation_id,digest,raw) VALUES($1,$2,$3)",
        &[
            &f.installation,
            &digest(b"different original bytes"),
            &f.raw,
        ],
    )
    .await
    .unwrap();
    assert!(load(&f.c, f.installation).await.is_err());
    assert!(validate_snapshot_bindings(&f.c).await.is_err());
    assert!(store(&f.c, f.installation, &f.manifest, Some(&f.raw))
        .await
        .is_err());
}

#[tokio::test]
async fn actual_postgres_http_call_freezes_operation_and_original_instance_generation() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    store(&f.c, f.installation, &f.manifest, Some(&f.raw))
        .await
        .unwrap();
    let registry = load(&f.c, f.installation).await.unwrap();
    let operation = registry
        .operations
        .iter()
        .find(|op| op.method == Method::POST)
        .unwrap();
    let deployment = Uuid::new_v4();
    let instance = Uuid::new_v4();
    let call = Uuid::new_v4();
    f.c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,'{}','disabled',now(),now())",&[&deployment,&f.owner,&f.installation]).await.unwrap();
    f.c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,1,0,'stopped','unknown','{}',now(),now())",&[&instance,&deployment]).await.unwrap();
    let binding = json!({"installation_id":f.installation,"deployment_id":deployment,"generation":1,"http_contract_digest":operation.contract_digest,"operation_id":operation.id,"method":operation.method.as_str(),"path":operation.path,"required_action":operation.route.action});
    let contract = json!({"name":format!("{}.http",f.manifest["plugin_id"].as_str().unwrap()),"version":f.manifest["version"],"schema_digest":operation.contract_digest});
    f.c.execute("INSERT INTO calls(id,owner_id,provider_instance_id,contract,input_digest,state,deadline_at,created_at,http_binding) VALUES($1,$2,$3,$4,$5,'succeeded',now(),now(),$6)",&[&call,&f.owner,&instance,&contract,&digest(b"{}"),&binding]).await.unwrap();
    validate_snapshot_bindings(&f.c).await.unwrap();
    let mut altered = binding.clone();
    altered["generation"] = json!(2);
    assert!(f
        .c
        .execute(
            "UPDATE calls SET http_binding=$2 WHERE id=$1",
            &[&call, &altered]
        )
        .await
        .is_err());
    f.c.execute("UPDATE calls SET contract='{}' WHERE id=$1", &[&call])
        .await
        .unwrap();
    assert!(validate_snapshot_bindings(&f.c).await.is_err());
}
