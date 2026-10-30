use crate::{contracts::validate,error::{Error,Result},db};
use serde_json::{json,Value};
use tokio_postgres::GenericClient;
use uuid::Uuid;

pub fn pool(config:&Value,cpu:i64,memory:i64)->Result<(i64,i64)>{
    validate("NodeConfig",config)?;
    let pc=cpu.checked_sub(config["kernel_cpu_millis"].as_i64().unwrap()).and_then(|v|v.checked_sub(config["system_cpu_millis"].as_i64().unwrap())).ok_or_else(Error::exhausted)?;
    let pm=memory.checked_sub(config["kernel_memory_bytes"].as_i64().unwrap()).and_then(|v|v.checked_sub(config["system_memory_bytes"].as_i64().unwrap())).ok_or_else(Error::exhausted)?;
    if pc<0||pm<0{return Err(Error::exhausted());}Ok((pc,pm))
}

pub fn host_capacity()->Result<(i64,i64)>{
    let cpu=std::thread::available_parallelism()?.get() as i64*1000;
    let mem=std::fs::read_to_string("/proc/meminfo")?;
    let kb=mem.lines().find(|l|l.starts_with("MemTotal:")).and_then(|l|l.split_whitespace().nth(1)).and_then(|s|s.parse::<i64>().ok()).ok_or_else(||Error::unavailable("Cannot discover host memory"))?;
    Ok((cpu,kb.checked_mul(1024).ok_or_else(Error::exhausted)?))
}

pub async fn initialize(c:&impl GenericClient,config:&Value,cpu:i64,memory:i64)->Result<()> {
    pool(config,cpu,memory)?;
    c.execute("INSERT INTO node_budget(id,config,host_cpu_millis,host_memory_bytes,created_at,updated_at) VALUES($1,$2,$3,$4,now(),now()) ON CONFLICT(singleton) DO NOTHING",&[&Uuid::new_v4(),&config,&cpu,&memory]).await?;
    let row=c.query_one("SELECT * FROM node_budget FOR UPDATE",&[]).await?;
    let existing:Value=row.get("config");let(pcpu,pmem)=pool(&existing,cpu,memory)?;
    if row.get::<_,i64>("reserved_cpu_millis")>pcpu||row.get::<_,i64>("reserved_memory_bytes")>pmem{return Err(Error::exhausted());}
    c.execute("UPDATE node_budget SET host_cpu_millis=$1,host_memory_bytes=$2,admission_enabled=false,revision=revision+1,updated_at=now()",&[&cpu,&memory]).await?;Ok(())
}

// Caller must use a transaction. All writers lock budget before instance/reservation.
pub async fn reserve(c:&impl GenericClient,instance:Uuid)->Result<()> {
    let budget=c.query_one("SELECT * FROM node_budget FOR UPDATE",&[]).await?;
    if !budget.get::<_,bool>("admission_enabled"){return Err(Error::unavailable("Admission is disabled during reconciliation"));}
    let row=c.query_one("SELECT i.*,d.spec,d.archived_at FROM instances i JOIN deployments d ON d.id=i.deployment_id WHERE i.id=$1 FOR UPDATE OF i,d",&[&instance]).await?;
    let limits:Value=row.get("limits");validate("ResourceLimits",&limits)?;
    let spec:Value=row.get("spec");
    if spec["limits"]!=limits||spec["enabled"]!=true||row.get::<_,Option<chrono::DateTime<chrono::Utc>>>("archived_at").is_some(){return Err(Error::conflict("Instance does not match an enabled deployment"));}
    let cpu=limits["cpu_millis"].as_i64().unwrap();let mem=limits["memory_bytes"].as_i64().unwrap();
    if let Some(r)=c.query_opt("SELECT * FROM reservations WHERE instance_id=$1 FOR UPDATE",&[&instance]).await?{
        if r.get::<_,String>("status")=="released"{return Err(Error::conflict("A released reservation cannot be revived"));}
        if r.get::<_,i64>("cpu_millis")!=cpu||r.get::<_,i64>("memory_bytes")!=mem{return Err(Error::conflict("Reservation limits differ"));}return Ok(());
    }
    let config:Value=budget.get("config");let(pc,pm)=pool(&config,budget.get("host_cpu_millis"),budget.get("host_memory_bytes"))?;
    let used_cpu=budget.get::<_,i64>("reserved_cpu_millis").checked_add(cpu).ok_or_else(Error::exhausted)?;
    let used_mem=budget.get::<_,i64>("reserved_memory_bytes").checked_add(mem).ok_or_else(Error::exhausted)?;
    if used_cpu>pc||used_mem>pm{return Err(Error::exhausted());}
    c.execute("INSERT INTO reservations(id,instance_id,kind,cpu_millis,memory_bytes,status,created_at) VALUES($1,$2,'instance',$3,$4,'reserved',now())",&[&Uuid::new_v4(),&instance,&cpu,&mem]).await?;
    c.execute("UPDATE node_budget SET reserved_cpu_millis=$1,reserved_memory_bytes=$2,revision=revision+1,updated_at=now()",&[&used_cpu,&used_mem]).await?;
    db::outbox(c,"instance.reserved",instance,&json!({})).await
}

// Runtime must have positively observed absence of the task and its cgroup processes.
pub async fn release_stopped(c:&impl GenericClient,instance:Uuid)->Result<()> {
    c.query_one("SELECT id FROM node_budget FOR UPDATE",&[]).await?;
    let i=c.query_one("SELECT phase FROM instances WHERE id=$1 FOR UPDATE",&[&instance]).await?;
    if i.get::<_,String>(0)!="stopped"{return Err(Error::conflict("Instance has not been confirmed stopped"));}
    if let Some(r)=c.query_opt("SELECT * FROM reservations WHERE instance_id=$1 FOR UPDATE",&[&instance]).await?{
        if r.get::<_,String>("status")!="released"{
            let cpu:i64=r.get("cpu_millis");let memory:i64=r.get("memory_bytes");
            c.execute("UPDATE node_budget SET reserved_cpu_millis=reserved_cpu_millis-$1,reserved_memory_bytes=reserved_memory_bytes-$2,revision=revision+1,updated_at=now()",&[&cpu,&memory]).await?;
            c.execute("UPDATE reservations SET status='released' WHERE instance_id=$1",&[&instance]).await?;
        }
    }Ok(())
}

pub async fn status(c:&impl GenericClient)->Result<Value>{
    let r=c.query_one("SELECT * FROM node_budget",&[]).await?;let config:Value=r.get("config");let cpu:i64=r.get("host_cpu_millis");let mem:i64=r.get("host_memory_bytes");let(pc,pm)=pool(&config,cpu,mem)?;
    let mut v=db::stamp(&r);for(k,val)in[("config",config),("host_cpu_millis",json!(cpu)),("host_memory_bytes",json!(mem)),("plugin_cpu_pool_millis",json!(pc)),("plugin_memory_pool_bytes",json!(pm)),("reserved_cpu_millis",json!(r.get::<_,i64>("reserved_cpu_millis"))),("reserved_memory_bytes",json!(r.get::<_,i64>("reserved_memory_bytes"))),("admission_enabled",json!(r.get::<_,bool>("admission_enabled")))]{v[k]=val;}validate("NodeStatus",&v)?;Ok(v)
}

#[cfg(test)]mod tests{use super::*;#[test]fn reservations_never_borrow_kernel_or_system(){let c=json!({"kernel_cpu_millis":4000,"kernel_memory_bytes":17179869184u64,"system_cpu_millis":1000,"system_memory_bytes":1073741824u64,"shutdown_grace_ms":30000});assert_eq!(pool(&c,8000,34359738368).unwrap(),(3000,16106127360));assert!(pool(&c,4000,34359738368).is_err());assert!(pool(&c,8000,17179869184).is_err());}}
