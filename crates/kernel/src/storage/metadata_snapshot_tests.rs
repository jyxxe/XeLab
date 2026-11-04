//! These round trips use real PostgreSQL COPY, target constraints and policy.
//! They are metadata components, not a whole-platform/off-site acceptance.
use super::*;
use crate::{auth, backups::tests::Fixture, db::Db};

const PASSWORD: &str = "fresh offline metadata restore password";

async fn target(backup: Uuid) -> (Db, Client, Uuid) {
    let base = std::env::var("XELAB_TEST_DATABASE_URL").unwrap();
    let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
    let name = format!("metadata_restore_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let db = Db::new(format!("{base} dbname={name}")).unwrap();
    db.migrate().await.unwrap();
    let c = db.connect().await.unwrap();
    let recovery = Uuid::new_v4();
    c.execute("INSERT INTO platform_recovery_state(recovery_id,backup_id,phase) VALUES($1,$2,'importing')",&[&recovery,&backup]).await.unwrap();
    (db, c, recovery)
}
async fn empty(c: &mut Client) {
    let tx = c.transaction().await.unwrap();
    for table in db::snapshot_tables() {
        assert!(
            !tx.query_one(
                &format!("SELECT EXISTS(SELECT 1 FROM {})", relation(table)),
                &[]
            )
            .await
            .unwrap()
            .get::<_, bool>(0),
            "{table} survived a failed import"
        );
    }
    assert!(tx
        .query_one(
            "SELECT phase='importing' AND metadata_summary IS NULL FROM platform_recovery_state",
            &[]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn actual_postgres_snapshot_preserves_bounded_http_contract_but_resets_live_approval() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut source = f.app.db.connect().await.unwrap();
    let owner: Uuid = source
        .query_one("SELECT created_by FROM backups WHERE id=$1", &[&f.backup])
        .await
        .unwrap()
        .get(0);
    let publisher = Uuid::new_v4();
    let (_directory, mut manifest, mut raw) =
        crate::http_contracts::tests::fixture_manifest(publisher);
    // Exercise the supported maximum bytea payload. Binary COPY can carry this
    // bounded member without confusing its doubled JSON hex projection for an
    // oversized row; unrelated tables retain their existing row-size checks.
    raw.resize(crate::plugin_http::MAXIMUM_CONTRACT_BYTES, b' ');
    let hash = crate::contracts::digest(&raw);
    for route in manifest["routes"].as_array_mut().unwrap() {
        if route["kind"] == "http" {
            route["http_contract_digest"] = serde_json::json!(hash);
        }
    }
    for skill in manifest["skills"].as_array_mut().unwrap() {
        if skill["target"]["kind"] == "http" {
            skill["target"]["http_contract_digest"] = serde_json::json!(hash);
        }
    }
    let installation = Uuid::new_v4();
    let deployment = Uuid::new_v4();
    source.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'snapshot HTTP publisher',$2,'component-only','trusted',now(),now())",&[&publisher,&format!("http-{publisher}")]).await.unwrap();
    source.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'installed',now(),now())",&[&installation,&manifest["plugin_id"].as_str().unwrap(),&manifest["version"].as_str().unwrap(),&publisher,&crate::contracts::digest(b"component-only image"),&crate::contracts::digest(&serde_json::to_vec(&manifest).unwrap()),&manifest]).await.unwrap();
    crate::http_contracts::store(&source, installation, &manifest, Some(&raw))
        .await
        .unwrap();
    source.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,'{\"enabled\":false}','disabled',now(),now())",&[&deployment,&owner,&installation]).await.unwrap();
    let credential: Uuid = source
        .query_one(
            "SELECT id FROM credentials WHERE owner_id=$1 LIMIT 1",
            &[&owner],
        )
        .await
        .unwrap()
        .get(0);
    source.execute("INSERT INTO plugin_http_approvals(deployment_id,credential_id,peer,approval_digest,source_paths) VALUES($1,$2,'127.0.0.1',$3,'[]')",&[&deployment,&credential,&crate::contracts::digest(b"component-only approval")]).await.unwrap();
    let mut bytes = Vec::new();
    let summary = export_inner(&mut source, &mut bytes).await.unwrap();
    let (_, mut destination, recovery) = target(f.backup).await;
    import_inner(
        &mut destination,
        &mut bytes.as_slice(),
        &summary,
        recovery,
        owner,
        PASSWORD,
    )
    .await
    .unwrap();
    let restored = destination
        .query_one(
            "SELECT digest,raw FROM plugin_http_contracts WHERE installation_id=$1",
            &[&installation],
        )
        .await
        .unwrap();
    assert_eq!(restored.get::<_, String>("digest"), hash);
    assert_eq!(restored.get::<_, Vec<u8>>("raw"), raw);
    assert!(crate::http_contracts::load(&destination, installation)
        .await
        .is_ok());
    assert!(!destination
        .query_one("SELECT EXISTS(SELECT 1 FROM plugin_http_approvals)", &[])
        .await
        .unwrap()
        .get::<_, bool>(0));
    assert_eq!(
        destination
            .query_one("SELECT status FROM publishers WHERE id=$1", &[&publisher])
            .await
            .unwrap()
            .get::<_, String>(0),
        "revoked"
    );
    assert_eq!(
        destination
            .query_one(
                "SELECT spec->>'enabled' FROM deployments WHERE id=$1",
                &[&deployment]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
        "false"
    );
    assert!(destination
        .execute(
            "DELETE FROM plugin_http_contracts WHERE installation_id=$1",
            &[&installation]
        )
        .await
        .is_err());
}

#[tokio::test]
async fn actual_postgres_snapshot_round_trip_and_late_failure_rollback() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut source = f.app.db.connect().await.unwrap();
    let owner: Uuid = source
        .query_one("SELECT created_by FROM backups WHERE id=$1", &[&f.backup])
        .await
        .unwrap()
        .get(0);
    let literal = "snapshot text: '); DROP TABLE accounts; --\n恢复";
    source
        .execute(
            "UPDATE accounts SET display_name=$2 WHERE id=$1",
            &[&owner, &literal],
        )
        .await
        .unwrap();
    // Nontransactional sequence high water exceeds all live project rows and
    // must survive the restore too; it cannot be inferred only from allocations.
    source
        .query_one("SELECT setval('volume_project_ids',9000,true)", &[])
        .await
        .unwrap();
    let mut bytes = Vec::new();
    let summary = export_inner(&mut source, &mut bytes).await.unwrap();
    assert_eq!(summary.bytes, bytes.len() as u64);
    assert!(summary.rows > 0);
    let (db, mut destination, recovery) = target(f.backup).await;
    assert!(db.check().await.is_err());
    let before = foreign_keys(&destination.transaction().await.unwrap())
        .await
        .unwrap();
    let mut corrupt = bytes.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    assert!(import_inner(
        &mut destination,
        &mut corrupt.as_slice(),
        &summary,
        recovery,
        owner,
        PASSWORD
    )
    .await
    .is_err());
    empty(&mut destination).await;
    let after = foreign_keys(&destination.transaction().await.unwrap())
        .await
        .unwrap();
    assert_eq!(before, after, "failed import must restore every target FK");
    let mut mismatch = summary.clone();
    mismatch.sha256 = "0".repeat(64);
    assert!(import_inner(
        &mut destination,
        &mut bytes.as_slice(),
        &mismatch,
        recovery,
        owner,
        PASSWORD
    )
    .await
    .is_err());
    empty(&mut destination).await;
    assert!(import_inner(
        &mut destination,
        &mut &bytes[..bytes.len() - 10],
        &summary,
        recovery,
        owner,
        PASSWORD
    )
    .await
    .is_err());
    empty(&mut destination).await;
    let restored = import_inner(
        &mut destination,
        &mut bytes.as_slice(),
        &summary,
        recovery,
        owner,
        PASSWORD,
    )
    .await
    .unwrap();
    assert_eq!(restored, summary);
    assert_eq!(
        destination
            .query_one("SELECT display_name FROM accounts WHERE id=$1", &[&owner])
            .await
            .unwrap()
            .get::<_, String>(0),
        literal
    );
    assert!(
        db.check().await.is_err(),
        "filesystem rebuilding is still outstanding"
    );
    let row = destination
        .query_one(
            "SELECT phase,metadata_summary,recovery_owner_id FROM platform_recovery_state",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "rebuilding");
    assert_eq!(
        row.get::<_, serde_json::Value>(1),
        serde_json::to_value(&summary).unwrap()
    );
    assert_eq!(row.get::<_, Uuid>(2), owner);
    let peer = "127.0.0.1".parse().unwrap();
    assert!(f
        .app
        .auth
        .authenticate(&destination, &f.token, peer, Uuid::new_v4())
        .await
        .is_err());
    let issued = f
        .app
        .auth
        .login(&destination, owner, PASSWORD.into(), peer, Uuid::new_v4())
        .await
        .unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(
            &destination,
            issued["access_token"].as_str().unwrap(),
            peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    auth::authorize(
        &destination,
        &ctx,
        "publisher.trust",
        &auth::Resource::new("publisher", Uuid::new_v4(), None),
    )
    .await
    .unwrap();
    let high: i64 = destination
        .query_one("SELECT nextval('volume_project_ids')", &[])
        .await
        .unwrap()
        .get(0);
    assert!(high > 9000);
    assert_eq!(
        foreign_keys(&destination.transaction().await.unwrap())
            .await
            .unwrap(),
        before
    );
    // A completed metadata import cannot be replayed into its nonempty target.
    assert!(import_inner(
        &mut destination,
        &mut bytes.as_slice(),
        &summary,
        recovery,
        owner,
        PASSWORD
    )
    .await
    .is_err());
}

#[tokio::test]
async fn actual_postgres_rejects_incompatible_headers_and_toasted_oversize_rows() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut source = f.app.db.connect().await.unwrap();
    let owner: Uuid = source
        .query_one("SELECT created_by FROM backups WHERE id=$1", &[&f.backup])
        .await
        .unwrap()
        .get(0);
    let mut bytes = Vec::new();
    let summary = export_inner(&mut source, &mut bytes).await.unwrap();
    let (_, mut destination, recovery) = target(f.backup).await;
    let size = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
    for change in 0..3 {
        let mut header: Header = serde_json::from_slice(&bytes[12..12 + size]).unwrap();
        match change {
            0 => header.postgres_major += 1,
            1 => header.tables[0].name = "accounts; DROP TABLE accounts; --".into(),
            _ => header.tables[0].columns[0].data_type = "unsupported source type".into(),
        }
        let json = serde_json::to_vec(&header).unwrap();
        let mut altered = Vec::new();
        altered.extend(MAGIC);
        altered.extend((json.len() as u32).to_be_bytes());
        altered.extend(json);
        altered.extend(&bytes[12 + size..]);
        let error = import_inner(
            &mut destination,
            &mut altered.as_slice(),
            &summary,
            recovery,
            owner,
            PASSWORD,
        )
        .await
        .unwrap_err();
        assert_eq!(error.status.as_u16(), 409);
        empty(&mut destination).await;
    }
    let mut oversized_frame = bytes.clone();
    oversized_frame[12 + size..16 + size].copy_from_slice(&((CHUNK + 1) as u32).to_be_bytes());
    assert!(import_inner(
        &mut destination,
        &mut oversized_frame.as_slice(),
        &summary,
        recovery,
        owner,
        PASSWORD
    )
    .await
    .is_err());
    empty(&mut destination).await;
    // pg_column_size alone would accept a huge compressed/toasted value. Bound
    // its expanded representation before the COPY protocol can queue that row.
    source.batch_execute("UPDATE installations SET manifest=jsonb_build_object('oversized',repeat('x',9*1024*1024))").await.unwrap();
    let mut rejected = Vec::new();
    assert!(export_inner(&mut source, &mut rejected).await.is_err());
    assert!(rejected.is_empty());
}

#[tokio::test]
async fn actual_postgres_import_validates_target_foreign_keys_before_commit() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut source = f.app.db.connect().await.unwrap();
    let owner: Uuid = source
        .query_one("SELECT created_by FROM backups WHERE id=$1", &[&f.backup])
        .await
        .unwrap()
        .get(0);
    // A structurally valid COPY stream with a broken reference must not commit.
    // The source constraint change never enters the snapshot or the target DDL.
    source
        .batch_execute("ALTER TABLE backups DROP CONSTRAINT backups_created_by_fkey")
        .await
        .unwrap();
    source
        .execute(
            "UPDATE backups SET created_by=$1 WHERE id=$2",
            &[&Uuid::new_v4(), &f.backup],
        )
        .await
        .unwrap();
    let mut bytes = Vec::new();
    let summary = export_inner(&mut source, &mut bytes).await.unwrap();
    let (_, mut destination, recovery) = target(f.backup).await;
    let before = foreign_keys(&destination.transaction().await.unwrap())
        .await
        .unwrap();
    assert!(import_inner(
        &mut destination,
        &mut bytes.as_slice(),
        &summary,
        recovery,
        owner,
        PASSWORD
    )
    .await
    .is_err());
    empty(&mut destination).await;
    assert_eq!(
        foreign_keys(&destination.transaction().await.unwrap())
            .await
            .unwrap(),
        before
    );
}
