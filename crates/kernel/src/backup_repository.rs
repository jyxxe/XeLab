//! A database cannot silently adopt another database's remote backup prefix.
//! Registration precedes the conditional marker; interrupted preparation is
//! reconciled from exactly that persisted repository UUID.
use crate::{
    config::Config,
    contracts::{digest, strict_json},
    db::Db,
    error::{Error, Result},
    storage::{backup_crypto, s3},
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tokio_postgres::GenericClient;
use uuid::Uuid;
use zeroize::Zeroizing;
const MARKER: &str = "xelab-repository.identity";
const MANIFEST_LIMIT: usize = 1024 * 1024;
pub struct Settings {
    uri: String,
    region: String,
    credentials: PathBuf,
}
impl Settings {
    pub fn from_host(config: &Config) -> Result<Self> {
        let uri = config.text("storage", "off_host_backup_uri").to_owned();
        let region = std::env::var("AWS_REGION")
            .map_err(|_| Error::unavailable("Backup AWS region is not configured"))?;
        let credentials = std::env::var_os("XELAB_BACKUP_CREDENTIALS_FILE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| Error::unavailable("Backup credential file is not configured"))?;
        s3::Location::aws(&uri, &region)?;
        Ok(Self {
            uri,
            region,
            credentials,
        })
    }
    pub fn client(&self) -> Result<Arc<s3::Client>> {
        Ok(Arc::new(s3::Client::new(
            s3::Location::aws(&self.uri, &self.region)?,
            s3::Credentials::load(&self.credentials)?,
        )?))
    }
    pub async fn register(&self, c: &(impl GenericClient + Sync)) -> Result<Uuid> {
        c.execute("INSERT INTO backup_repositories(id,location,region,state) VALUES($1,$2,$3,'preparing') ON CONFLICT(singleton) DO NOTHING", &[&Uuid::new_v4(), &self.uri, &self.region]).await?;
        let row = c
            .query_one(
                "SELECT id,location,region FROM backup_repositories FOR SHARE",
                &[],
            )
            .await?;
        if row.get::<_, String>("location") != self.uri
            || row.get::<_, String>("region") != self.region
        {
            return Err(Error::unavailable(
                "Backup repository differs from durable registration",
            ));
        }
        Ok(row.get("id"))
    }
    pub async fn bind(&self, db: &Db, repository: Uuid) -> Result<Repository> {
        Repository::bind(db, repository, &self.uri, &self.region, self.client()?).await
    }
}
pub struct Repository {
    pub id: Uuid,
    pub objects: Arc<s3::Client>,
}
impl Repository {
    async fn bind(
        db: &Db,
        id: Uuid,
        uri: &str,
        region: &str,
        client: Arc<s3::Client>,
    ) -> Result<Self> {
        let c = db.connect().await?;
        let row = c
            .query_opt(
                "SELECT location,region,state FROM backup_repositories WHERE id=$1",
                &[&id],
            )
            .await?
            .ok_or_else(|| Error::unavailable("Backup repository is not registered"))?;
        if row.get::<_, String>("location") != uri || row.get::<_, String>("region") != region {
            return Err(Error::unavailable("Backup repository registration differs"));
        }
        let preparing = row.get::<_, String>("state") == "preparing";
        let expected = json!({"format":1,"repository_id":id});
        let marker = client.get_optional(MARKER).await?;
        let marker = match marker {
            Some(value) => value,
            None if preparing => {
                // A failed PUT can already have published. A GET of the exact
                // expected marker is the only acceptance condition.
                let _ = client.put(MARKER, serde_json::to_vec(&expected)?).await;
                client.get(MARKER).await?
            }
            None => {
                return Err(Error::unavailable(
                    "Ready backup repository marker is missing",
                ))
            }
        };
        let raw = s3::bounded(marker, 4096).await?;
        if strict_json(&raw, 4096)? != expected {
            return Err(Error::unavailable(
                "Backup prefix belongs to another repository",
            ));
        }
        c.execute("UPDATE backup_repositories SET state='ready' WHERE id=$1 AND location=$2 AND region=$3", &[&id,&uri,&region]).await?;
        Ok(Self {
            id,
            objects: Arc::new(client.scoped(&format!("repositories/{id}"))?),
        })
    }
    pub async fn write_manifest(
        &self,
        backup: Uuid,
        object: Uuid,
        master: &[u8; 32],
        manifest: &Value,
    ) -> Result<String> {
        let plain = Zeroizing::new(serde_json::to_vec(manifest)?);
        if plain.len() > MANIFEST_LIMIT {
            return Err(Error::exhausted());
        }
        let expected = digest(&plain);
        let mut sealed = Vec::new();
        backup_crypto::seal(
            plain.as_slice(),
            &mut sealed,
            master,
            backup,
            object,
            MANIFEST_LIMIT as u64,
        )?;
        let key = format!("{backup}/{object}.sealed");
        let _ = self.objects.put(&key, sealed).await;
        let actual = self
            .read_manifest(backup, object, master, &expected)
            .await?;
        if actual != *manifest {
            return Err(Error::unavailable("Backup manifest readback differs"));
        }
        Ok(expected)
    }
    pub async fn read_manifest(
        &self,
        backup: Uuid,
        object: Uuid,
        master: &[u8; 32],
        expected: &str,
    ) -> Result<Value> {
        let key = format!("{backup}/{object}.sealed");
        let sealed = s3::bounded(self.objects.get(&key).await?, MANIFEST_LIMIT + 4096).await?;
        let mut plain = Zeroizing::new(Vec::new());
        backup_crypto::open(
            sealed.as_slice(),
            &mut *plain,
            master,
            backup,
            object,
            MANIFEST_LIMIT as u64,
        )?;
        if digest(&plain) != expected {
            return Err(Error::unavailable("Backup manifest digest differs"));
        }
        strict_json(&plain, MANIFEST_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn database(base: &str) -> Db {
        let admin = Db::new(base.to_owned()).unwrap().connect().await.unwrap();
        let name = format!("repository_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let db = Db::new(format!("{base} dbname={name}")).unwrap();
        db.migrate().await.unwrap();
        db.migrate().await.unwrap();
        db.check().await.unwrap();
        db
    }
    #[tokio::test]
    async fn actual_s3_namespace_binding_recovery_and_encrypted_manifest() {
        let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: repository test requires PostgreSQL");
            return;
        };
        let Some((client, root)) = s3::tests::fixture().await else {
            return;
        };
        let uri = client
            .reference("marker")
            .unwrap()
            .strip_suffix("marker")
            .unwrap()
            .to_owned();
        let settings = Settings {
            uri: uri.clone(),
            region: "us-east-1".into(),
            credentials: root.path().join("s3.json"),
        };
        let client = Arc::new(client);
        let db = database(&base).await;
        let mut c = db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let repository = settings.register(&tx).await.unwrap();
        tx.commit().await.unwrap();
        let repo = Repository::bind(&db, repository, &uri, "us-east-1", client.clone())
            .await
            .unwrap();
        assert!(!repo.objects.off_host());
        assert!(repo
            .objects
            .reference("sample.sealed")
            .unwrap()
            .contains(&format!("/repositories/{repository}/")));
        // The marker may have been published before the ready metadata commit.
        c.execute("UPDATE backup_repositories SET state='preparing'", &[])
            .await
            .unwrap();
        Repository::bind(&db, repository, &uri, "us-east-1", client.clone())
            .await
            .unwrap();
        assert_eq!(
            c.query_one("SELECT state FROM backup_repositories", &[])
                .await
                .unwrap()
                .get::<_, String>(0),
            "ready"
        );

        let other_db = database(&base).await;
        let mut other = other_db.connect().await.unwrap();
        let tx = other.transaction().await.unwrap();
        let other_id = settings.register(&tx).await.unwrap();
        tx.commit().await.unwrap();
        assert!(
            Repository::bind(&other_db, other_id, &uri, "us-east-1", client.clone())
                .await
                .is_err()
        );
        assert!(Repository::bind(
            &db,
            repository,
            "s3://other-bucket/node/",
            "us-east-1",
            client.clone()
        )
        .await
        .is_err());

        let backup = Uuid::new_v4();
        let object = Uuid::new_v4();
        let manifest = json!({"format":1,"backup_id":backup,"volumes":[],"master_key_reference":digest(&[31;32])});
        let hash = repo
            .write_manifest(backup, object, &[31; 32], &manifest)
            .await
            .unwrap();
        assert_eq!(
            repo.read_manifest(backup, object, &[31; 32], &hash)
                .await
                .unwrap(),
            manifest
        );
        assert_eq!(
            repo.write_manifest(backup, object, &[31; 32], &manifest)
                .await
                .unwrap(),
            hash
        );
        assert!(repo
            .write_manifest(
                backup,
                object,
                &[31; 32],
                &json!({"other":"cannot replace"})
            )
            .await
            .is_err());
        assert!(repo
            .read_manifest(backup, object, &[32; 32], &hash)
            .await
            .is_err());
        let raw = s3::bounded(
            repo.objects
                .get(&format!("{backup}/{object}.sealed"))
                .await
                .unwrap(),
            MANIFEST_LIMIT + 4096,
        )
        .await
        .unwrap();
        assert!(!raw
            .windows(b"master_key_reference".len())
            .any(|p| p == b"master_key_reference"));
        // Clean exactly our encrypted manifest and root marker; a ready DB may
        // not subsequently create/adopt a fresh marker in the emptied prefix.
        s3::tests::remove(
            &client,
            &[
                format!("repositories/{repository}/{backup}/{object}.sealed"),
                MARKER.into(),
            ],
        )
        .await;
        assert!(
            Repository::bind(&db, repository, &uri, "us-east-1", client.clone())
                .await
                .is_err()
        );
        s3::tests::cleanup(&client, &[]).await;
    }
}
