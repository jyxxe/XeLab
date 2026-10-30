use crate::{
    contracts::{digest, validate},
    error::{Error, Result},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_postgres::{Client, GenericClient, NoTls, Row};
use uuid::Uuid;

pub const BASELINE: &str = include_str!("../../../docs/contracts/metadata.sql");
pub const LEADER_LOCK: i64 = 0x58454c41424d3101;

#[derive(Clone)]
pub struct Db {
    dsn: Arc<str>,
}
impl Db {
    pub fn new(dsn: String) -> Result<Self> {
        let c: tokio_postgres::Config = dsn
            .parse()
            .map_err(|_| Error::invalid("Invalid PostgreSQL DSN"))?;
        for host in c.get_hosts() {
            match host {
                tokio_postgres::config::Host::Tcp(h)
                    if h.parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback()) =>
                {
                    ()
                }
                tokio_postgres::config::Host::Unix(_) => (),
                _ => {
                    return Err(Error::invalid(
                        "M1 PostgreSQL must use a local Unix socket or numeric loopback address",
                    ))
                }
            }
        }
        Ok(Self { dsn: dsn.into() })
    }
    pub async fn connect(&self) -> Result<Client> {
        let (c, connection) = tokio_postgres::connect(&self.dsn, NoTls).await?;
        tokio::spawn(async move {
            if connection.await.is_err() {
                tracing::error!(reason_code = "DATABASE_CONNECTION_LOST");
            }
        });
        Ok(c)
    }
    pub async fn check(&self) -> Result<()> {
        let c = self.connect().await?;
        verify_migration(&c).await
    }
    pub async fn migrate(&self) -> Result<()> {
        let mut c = self.connect().await?;
        let tx = c.transaction().await?;
        tx.query_one("SELECT pg_advisory_xact_lock($1)", &[&LEADER_LOCK])
            .await?;
        let present: Option<String> = tx
            .query_one("SELECT to_regclass('public.schema_migrations')::text", &[])
            .await?
            .get(0);
        if present.is_none() {
            let sql = BASELINE.replacen("BEGIN;", "", 1);
            let sql = sql
                .strip_suffix("COMMIT;\n")
                .or_else(|| sql.strip_suffix("COMMIT;"))
                .ok_or_else(|| Error::invalid("Unexpected baseline framing"))?;
            tx.batch_execute(sql).await?;
            tx.execute(
                "INSERT INTO schema_migrations(version,checksum,applied_at) VALUES(1,$1,now())",
                &[&digest(BASELINE.as_bytes())],
            )
            .await?;
        } else {
            verify_migration(&tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn leader(&self) -> Result<Client> {
        let c = self.connect().await?;
        let acquired: bool = c
            .query_one("SELECT pg_try_advisory_lock($1)", &[&LEADER_LOCK])
            .await?
            .get(0);
        if !acquired {
            return Err(Error::conflict(
                "A kernel controller already owns this database",
            ));
        }
        Ok(c)
    }
}

pub async fn verify_migration(c: &impl GenericClient) -> Result<()> {
    let r = c
        .query(
            "SELECT version,checksum FROM schema_migrations ORDER BY version",
            &[],
        )
        .await?;
    if r.len() != 1
        || r[0].get::<_, i64>(0) != 1
        || r[0].get::<_, String>(1) != digest(BASELINE.as_bytes())
    {
        return Err(Error::conflict(
            "Metadata migration version or checksum differs",
        ));
    }
    Ok(())
}

pub fn stamp(row: &Row) -> Value {
    json!({"id":row.get::<_,Uuid>("id"),"revision":row.get::<_,i64>("revision"),"created_at":row.get::<_,chrono::DateTime<chrono::Utc>>("created_at"),"updated_at":row.get::<_,chrono::DateTime<chrono::Utc>>("updated_at")})
}
pub fn project(row: &Row, fields: &[&str]) -> Result<Value> {
    let mut out = stamp(row);
    for name in fields {
        let val: Value = row
            .try_get(*name)
            .map_err(|_| Error::unavailable("Invalid metadata projection"))?;
        out[*name] = val;
    }
    Ok(out)
}

pub async fn audit(
    c: &impl GenericClient,
    subject: Uuid,
    credential: Option<Uuid>,
    request: Uuid,
    action: &str,
    resource: &Value,
    outcome: &str,
    reason: &str,
) -> Result<()> {
    c.execute("INSERT INTO audit_events(id,subject_id,credential_id,request_id,action,resource,outcome,policy_revision,reason_code,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,1,$8,now(),now())",&[&Uuid::new_v4(),&subject,&credential,&request,&action,&resource,&outcome,&reason]).await?;
    Ok(())
}
pub async fn outbox(c: &impl GenericClient, topic: &str, id: Uuid, payload: &Value) -> Result<()> {
    c.execute(
        "INSERT INTO outbox(id,topic,aggregate_id,payload,created_at) VALUES($1,$2,$3,$4,now())",
        &[&Uuid::new_v4(), &topic, &id, &payload],
    )
    .await?;
    Ok(())
}

pub async fn operation(
    c: &impl GenericClient,
    owner: Uuid,
    kind: &str,
    resource: Option<String>,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    c.execute("INSERT INTO operations(id,owner_id,kind,state,resource_id,created_at,updated_at) VALUES($1,$2,$3,'pending',$4,now(),now())",&[&id,&owner,&kind,&resource]).await?;
    Ok(id)
}
pub async fn operation_value(c: &impl GenericClient, id: Uuid) -> Result<Value> {
    let r = c
        .query_opt("SELECT * FROM operations WHERE id=$1", &[&id])
        .await?
        .ok_or_else(Error::not_found)?;
    let mut v = stamp(&r);
    v["kind"] = json!(r.get::<_, String>("kind"));
    v["state"] = json!(r.get::<_, String>("state"));
    v["progress_percent"] = json!(r.get::<_, i32>("progress_percent"));
    v["resource_id"] = json!(r.get::<_, Option<String>>("resource_id"));
    v["result"] = r.get::<_, Option<Value>>("result").unwrap_or(Value::Null);
    v["error"] = r.get::<_, Option<Value>>("error").unwrap_or(Value::Null);
    validate("Operation", &v)?;
    Ok(v)
}
pub async fn complete(
    c: &impl GenericClient,
    id: Uuid,
    result: std::result::Result<Value, Error>,
) -> Result<()> {
    let (state, value, error) = match result {
        Ok(v) => ("succeeded", Some(v), None),
        Err(e) => ("failed", None, Some(e.detail())),
    };
    c.execute("UPDATE operations SET state=$2,result=$3,error=$4,progress_percent=100,revision=revision+1,updated_at=now() WHERE id=$1 AND state IN ('pending','running')",&[&id,&state,&value,&error]).await?;
    Ok(())
}
