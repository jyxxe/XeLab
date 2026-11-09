//! Actual sequential migration, not a synthetic replacement schema.
use super::*;

#[tokio::test]
async fn actual_postgres_v15_oauth_migration_preserves_checksums_and_empty_restore_target() {
    let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: OAuth migration requires PostgreSQL");
        return;
    };
    let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
    let name = format!("oauth_migration_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let db = Db::new(format!("{base} dbname={name}")).unwrap();
    let c = db.connect().await.unwrap();
    for (index, sql) in MIGRATIONS.iter().take(15).enumerate() {
        c.batch_execute(sql).await.unwrap();
        c.execute(
            "INSERT INTO schema_migrations(version,checksum,applied_at) VALUES($1,$2,now())",
            &[&((index + 1) as i64), &digest(sql.as_bytes())],
        )
        .await
        .unwrap();
    }
    let before: Vec<(i64, String)> = c
        .query(
            "SELECT version,checksum FROM schema_migrations ORDER BY version",
            &[],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    assert!(db.check().await.is_err());
    db.migrate().await.unwrap();
    db.migrate().await.unwrap();
    db.check().await.unwrap();
    let versions: Vec<i64> = c
        .query("SELECT version FROM schema_migrations ORDER BY version", &[])
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(versions, (1..=SCHEMA_VERSION as i64).collect::<Vec<_>>());
    let after: Vec<(i64, String)> = c
        .query(
            "SELECT version,checksum FROM schema_migrations WHERE version<=15 ORDER BY version",
            &[],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    assert_eq!(before, after);
    assert_eq!(
        c.query_one(
            "SELECT checksum FROM schema_migrations WHERE version=16",
            &[]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        digest(OAUTH_STATE.as_bytes())
    );
    for table in snapshot_tables() {
        assert!(
            !c.query_one(&format!("SELECT EXISTS(SELECT 1 FROM {table})"), &[])
                .await
                .unwrap()
                .get::<_, bool>(0),
            "{table}: migrations must not seed a COPY import target"
        );
    }
    crate::oauth::snapshot::validate_snapshot(&c).await.unwrap();
    crate::oauth::snapshot::ensure_authority_reset(&c)
        .await
        .unwrap();
}
