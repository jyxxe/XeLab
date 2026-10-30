//! Privileged, disposable-host component test. It never lowers daemon preflight limits.
use axum::{body::{to_bytes, Body}, extract::ConnectInfo, http::Request, Router};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{pkcs8::EncodePublicKey, Signer, SigningKey};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use serde_json::{json, Value};
use std::{fs, net::SocketAddr, os::unix::fs::PermissionsExt, sync::{atomic::{AtomicBool, Ordering}, Arc}, time::Duration};
use tower::ServiceExt;
use uuid::Uuid;
use xelab_kernel::{admission, api::{router, App}, artifact, auth::{self, Auth}, config::Config, contracts, controller, db::Db, runtime::Runtime, tls};

async fn api(router: &Router, operation: &str, params: &[(&str, String)], token: Option<&str>, body: Value, key: &str, revision: Option<i64>) -> (u16, Value) {
    let mut selected = None;
    for (path, methods) in contracts::openapi()["paths"].as_object().unwrap() {
        for (method, spec) in methods.as_object().unwrap() {
            if spec["operationId"] == operation { selected = Some((path.clone(),method.to_uppercase())); }
        }
    }
    let (mut path, method) = selected.unwrap();
    for (name,value) in params { path = path.replace(&format!("{{{name}}}"),value); }
    let mut req = Request::builder().method(method.as_str()).uri(path).header("idempotency-key",key);
    if let Some(token) = token { req=req.header("authorization",format!("Bearer {token}")); }
    if let Some(revision)=revision { req=req.header("if-match",format!("\"{revision}\"")); }
    let bytes = if body.is_null() { Vec::new() } else { req=req.header("content-type","application/json"); serde_json::to_vec(&body).unwrap() };
    let mut req=req.body(Body::from(bytes)).unwrap();
    req.extensions_mut().insert(ConnectInfo("127.0.0.1:50000".parse::<SocketAddr>().unwrap()));
    let response=router.clone().oneshot(req).await.unwrap();
    let status=response.status().as_u16();
    let bytes=to_bytes(response.into_body(),16777216).await.unwrap();
    (status,if bytes.is_empty(){Value::Null}else{serde_json::from_slice(&bytes).unwrap()})
}

async fn operation(router: &Router, id: &str, token: &str) -> Value {
    for _ in 0..120 {
        let (status, value)=api(router,"getOperation",&[("operation_id",id.into())],Some(token),Value::Null,"poll",None).await;
        assert_eq!(status,200,"{value}");
        if value["state"]=="succeeded" { return value["result"].clone(); }
        assert_ne!(value["state"],"failed","{value}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("operation did not finish");
}

#[tokio::test]
#[ignore = "requires root, containerd, nftables and a disposable Linux host; see README"]
async fn signed_oci_plugin_runs_through_the_management_api_and_recovers() {
    assert_eq!(std::env::var("XELAB_OCI_TEST").as_deref(),Ok("1"));
    assert_eq!(unsafe{libc::geteuid()},0);
    xelab_kernel::init_crypto();
    let base=std::env::var("XELAB_TEST_DATABASE_URL").unwrap();
    let admin=Db::new(base.clone()).unwrap().connect().await.unwrap();
    admin.batch_execute("CREATE DATABASE xelab_oci").await.unwrap();
    let db=Db::new(format!("{base} dbname=xelab_oci")).unwrap();
    db.migrate().await.unwrap();
    let directory=tempfile::tempdir().unwrap();
    let root=directory.path();
    let key=KeyPair::generate().unwrap();
    let mut ca=CertificateParams::new(vec!["xelab-test-ca".into()]).unwrap();
    ca.is_ca=IsCa::Ca(BasicConstraints::Unconstrained);
    let cert=ca.self_signed(&key).unwrap();
    fs::write(root.join("ca.crt"),cert.pem()).unwrap();
    fs::write(root.join("ca.key"),key.serialize_pem()).unwrap();
    fs::set_permissions(root.join("ca.key"),fs::Permissions::from_mode(0o600)).unwrap();
    let t:toml::Value=toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
    let mut value=serde_json::to_value(t).unwrap();
    value["storage"]["root_path"]=json!(root.join("state"));
    value["security"]["workload_ca_cert_file"]=json!(root.join("ca.crt"));
    value["security"]["workload_ca_key_file"]=json!(root.join("ca.key"));
    value["runtime"]["namespace"]=json!("xelab-ci");
    value["runtime"]["network_cidr"]=json!("172.29.240.0/24");
    value["server"]["host_rpc_listen"]=json!("172.29.240.1:7443");
    let config=Config::from_value(value).unwrap();
    let runtime=Runtime::new(config.clone());
    // Hosted CI is smaller than the production 4 CPU + 16 GiB kernel reservation.
    // Exercise real per-instance cgroups separately, with a synthetic metadata pool.
    assert!(admission::pool(&config.value["resources"],4000,17179869184).is_err());
    fs::create_dir_all("/sys/fs/cgroup/xelab/plugins").unwrap();
    for path in ["/sys/fs/cgroup/cgroup.subtree_control","/sys/fs/cgroup/xelab/cgroup.subtree_control","/sys/fs/cgroup/xelab/plugins/cgroup.subtree_control"] { fs::write(path,"+cpu +memory +pids").unwrap(); }
    fs::write("/sys/fs/cgroup/xelab/plugins/memory.max","1073741824").unwrap();
    fs::write("/sys/fs/cgroup/xelab/plugins/cpu.max","200000 100000").unwrap();
    fs::create_dir_all(config.root()).unwrap();
    tls::issue(&config,None,&config.root().join("kernel-identity")).unwrap();
    let mut c=db.connect().await.unwrap();
    let tx=c.transaction().await.unwrap();
    let owner=auth::bootstrap(&tx,"OCI test owner","correct horse test password",&config.value["quota_defaults"]).await.unwrap();
    admission::initialize(&tx,&config.value["resources"],8000,34359738368).await.unwrap();
    tx.commit().await.unwrap();
    let publisher:Uuid="00000000-0000-4000-8000-000000000001".parse().unwrap();
    let signing=SigningKey::from_bytes(&[7;32]);
    let pem=signing.verifying_key().to_public_key_pem(Default::default()).unwrap();
    c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'fixture','fixture',$2,'trusted',now(),now())",&[&publisher,&pem]).await.unwrap();
    let raw=fs::read("examples/generated/xelab/plugin.json").unwrap();
    let manifest:Value=serde_json::from_slice(&raw).unwrap();
    let image=std::env::var("XELAB_TEST_IMAGE").unwrap();
    let envelope=artifact::signature_envelope(&manifest,artifact::image_ref(&image).unwrap(),&raw).unwrap();
    let install=json!({"publisher_id":publisher,"image":image,"signature":STANDARD.encode(signing.sign(envelope.as_bytes()).to_bytes())});
    let mut bad=install.clone();bad["signature"]=json!(STANDARD.encode([0;64]));
    assert!(artifact::Artifact::inspect(&runtime,&bad,&pem).await.is_err());
    let app=Arc::new(App{config:config.clone(),db:db.clone(),auth:Auth::new(vec![42;32]).unwrap(),runtime,leader:Arc::new(db.leader().await.unwrap()),ready:AtomicBool::new(false),shutdown:AtomicBool::new(false)});
    let mut worker=tokio::spawn(controller::run(app.clone()));
    let router=router(app.clone());
    let (status,session)=api(&router,"login",&[],None,json!({"account_id":owner,"password":"correct horse test password"}),"login",None).await;
    assert_eq!(status,200,"{session}");let token=session["access_token"].as_str().unwrap();
    let (status,installed)=api(&router,"installPlugin",&[],Some(token),install.clone(),"install-1",None).await;
    assert_eq!(status,202,"{installed}");
    let replay=api(&router,"installPlugin",&[],Some(token),install,"install-1",None).await;assert_eq!(replay.1["id"],installed["id"]);
    let installed=operation(&router,installed["id"].as_str().unwrap(),token).await;
    let limits=json!({"cpu_millis":500,"memory_bytes":134217728,"ephemeral_bytes":1048576,"pids":64});
    let (status,created)=api(&router,"createDeployment",&[],Some(token),json!({"installation_id":installed["installation_id"],"display_name":"echo","enabled":false,"limits":limits,"config":{},"volume_ids":[]}),"deploy-1",None).await;
    assert_eq!(status,202,"{created}");
    let deployment=operation(&router,created["id"].as_str().unwrap(),token).await["deployment_id"].as_str().unwrap().to_owned();
    let params=[("deployment_id",deployment.clone())];
    let approved=api(&router,"approvePlugin",&params,Some(token),json!({"rules":[],"egress":[]}),"approve-1",Some(1)).await;assert_eq!(approved.0,200,"{}",approved.1);
    let stale=api(&router,"setDeploymentEnabled",&params,Some(token),json!({"enabled":true}),"stale-enable",Some(1)).await;assert_eq!(stale.0,412,"{}",stale.1);
    let enabled=api(&router,"setDeploymentEnabled",&params,Some(token),json!({"enabled":true}),"enable-1",Some(2)).await;assert_eq!(enabled.0,202,"{}",enabled.1);
    operation(&router,enabled.1["id"].as_str().unwrap(),token).await;
    let input=json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":"hello OCI"},"provider_deployment_id":deployment,"timeout_ms":10000});
    let called=api(&router,"invokeCapability",&[],Some(token),input.clone(),"invoke-1",None).await;assert_eq!(called.0,200,"{}",called.1);assert_eq!(called.1["output"]["message"],"hello OCI");
    let replay=api(&router,"invokeCapability",&[],Some(token),input.clone(),"invoke-1",None).await;assert_eq!(replay.1,called.1);
    let mut different=input.clone();different["input"]["message"]=json!("different");assert_eq!(api(&router,"invokeCapability",&[],Some(token),different,"invoke-1",None).await.0,409);
    let call_id=called.1["call_id"].as_str().unwrap();assert_eq!(api(&router,"getCall",&[("call_id",call_id.into())],Some(token),Value::Null,"read",None).await.1["state"],"succeeded");
    let instance:Uuid=called.1["provider_instance_id"].as_str().unwrap().parse().unwrap();
    // Expected generation is part of the server certificate name; a different one fails closed.
    let instance_row=c.query_one("SELECT generation FROM instances WHERE id=$1",&[&instance]).await.unwrap();
    assert!(tls::channel(&config,instance,instance_row.get::<_,i64>(0)+1,&app.runtime.address(instance).unwrap()).await.is_err());
    let identity=config.root().join("runtime").join(instance.to_string()).join("identity");
    let before=fs::read(identity.join("tls.key")).unwrap();tls::issue(&config,Some((instance,instance_row.get(0))),&identity).unwrap();assert_eq!(fs::read(identity.join("tls.key")).unwrap(),before);
    // Restart the controller; recovery fences the old container before restoring capacity.
    worker.abort();let _=worker.await;app.ready.store(false,Ordering::SeqCst);
    worker=tokio::spawn(controller::run(app.clone()));
    for _ in 0..100 { if app.ready.load(Ordering::SeqCst) && api(&router,"invokeCapability",&[],Some(token),input.clone(),"after-restart",None).await.0==200 {break;}tokio::time::sleep(Duration::from_millis(500)).await; }
    assert_eq!(api(&router,"invokeCapability",&[],Some(token),input,"after-restart",None).await.0,200);
    assert_eq!(c.query_one("SELECT reserved_cpu_millis FROM node_budget",&[]).await.unwrap().get::<_,i64>(0),500);
    let deployment_value=api(&router,"getDeployment",&params,Some(token),Value::Null,"read",None).await.1;
    let disabled=api(&router,"setDeploymentEnabled",&params,Some(token),json!({"enabled":false}),"disable-1",deployment_value["revision"].as_i64()).await;assert_eq!(disabled.0,202,"{}",disabled.1);
    operation(&router,disabled.1["id"].as_str().unwrap(),token).await;
    assert!(!app.runtime.running(instance).await.unwrap());
    assert_eq!(c.query_one("SELECT reserved_cpu_millis FROM node_budget",&[]).await.unwrap().get::<_,i64>(0),0);
    let revoke=api(&router,"revokeSession",&[("session_id",session["session"]["id"].as_str().unwrap().into())],Some(token),Value::Null,"revoke-1",Some(1)).await;assert_eq!(revoke.0,204,"{}",revoke.1);
    assert_eq!(api(&router,"getNode",&[],Some(token),Value::Null,"read",None).await.0,401);
    app.shutdown.store(true,Ordering::SeqCst);worker.abort();
}
