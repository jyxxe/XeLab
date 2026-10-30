use crate::{admission,config::{Config,require_host_admin},contracts::{digest,strict_json},error::{Error,Result},tls};
use serde_json::{json,Value};
use std::{fs,net::Ipv4Addr,os::unix::fs::{MetadataExt,PermissionsExt},path::{Path,PathBuf},process::Stdio,time::Duration};
use tokio::{io::AsyncReadExt,process::Command};
use uuid::Uuid;

#[derive(Clone)]pub struct Runtime{pub config:Config}
pub struct Observation{pub running:bool,pub address:String}

fn binary(name:&str)->Result<PathBuf>{
    for root in ["/usr/sbin","/usr/bin","/sbin","/bin","/usr/local/bin"]{
        let p=Path::new(root).join(name);if !p.exists(){continue;}let p=p.canonicalize()?;let m=p.metadata()?;
        if m.uid()==0&&m.mode()&0o022==0&&m.is_file(){return Ok(p);}
    }Err(Error::unavailable("Required trusted host binary is unavailable"))
}

async fn command(name:&str,args:&[String],deadline:Duration)->Result<Vec<u8>>{
    let mut child=Command::new(binary(name)?).args(args).env_clear().env("PATH","/usr/sbin:/usr/bin:/sbin:/bin").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn()?;
    let mut stdout=child.stdout.take().ok_or_else(||Error::unavailable("Cannot capture host command"))?;
    let work=async{let mut out=Vec::new();(&mut stdout).take(2097153).read_to_end(&mut out).await?;if out.len()>2097152{return Err(Error::unavailable("Host command exceeded output limit"));}let status=child.wait().await?;if !status.success(){return Err(Error::unavailable("Host adapter command failed"));}Ok(out)};
    tokio::time::timeout(deadline,work).await.map_err(|_|Error::unavailable("Host adapter timed out"))?
}
fn strs(args:&[&str])->Vec<String>{args.iter().map(|s|s.to_string()).collect()}

impl Runtime{
    pub fn new(config:Config)->Self{Self{config}}
    pub async fn ctr(&self,args:&[String])->Result<Vec<u8>>{
        let mut all=strs(&["--address",self.config.text("runtime","socket"),"--namespace",self.config.text("runtime","namespace")]);all.extend_from_slice(args);command("ctr",&all,Duration::from_secs(60)).await
    }
    pub async fn preflight(&self,cpu:i64,memory:i64)->Result<()> {
        require_host_admin()?;admission::pool(&self.config.value["resources"],cpu,memory)?;
        for name in ["ctr","ip","nft"]{binary(name)?;}
        if !fs::read_to_string("/sys/fs/cgroup/cgroup.controllers")?.split_whitespace().any(|x|x=="memory"){return Err(Error::unavailable("cgroups v2 memory controller is unavailable"));}
        self.ctr(&strs(&["version"])).await?;
        fs::create_dir_all(self.config.root())?;fs::set_permissions(self.config.root(),fs::Permissions::from_mode(0o700))?;
        let root=Path::new("/sys/fs/cgroup/xelab");fs::create_dir_all(root)?;
        fs::write("/sys/fs/cgroup/cgroup.subtree_control","+cpu +memory +pids")?;
        fs::write(root.join("cgroup.subtree_control"),"+cpu +memory +pids")?;
        fs::create_dir_all(root.join("kernel"))?;fs::create_dir_all(root.join("plugins"))?;
        fs::write(root.join("kernel/memory.min"),self.config.value["resources"]["kernel_memory_bytes"].to_string())?;
        fs::write(root.join("kernel/cgroup.procs"),std::process::id().to_string())?;
        let(pc,pm)=admission::pool(&self.config.value["resources"],cpu,memory)?;
        if pc>0{fs::write(root.join("plugins/cpu.max"),format!("{} 100000",pc*100))?;}
        fs::write(root.join("plugins/memory.max"),pm.to_string())?;
        fs::write(root.join("plugins/memory.swap.max"),"0")?;
        fs::write(root.join("plugins/cgroup.subtree_control"),"+cpu +memory +pids")?;
        // No writable persistent volumes are admitted in M1; temporary writes use bounded tmpfs.
        Ok(())
    }
    pub async fn verify_image(&self,image:&str,expected:&str)->Result<()> {
        let content=self.ctr(&strs(&["content","get",expected])).await?;
        if digest(&content)!=expected{return Err(Error::invalid("Image manifest digest differs"));}
        let manifest=strict_json(&content,2097152)?;
        if !matches!(manifest["mediaType"].as_str(),Some("application/vnd.oci.image.manifest.v1+json"|"application/vnd.docker.distribution.manifest.v2+json")){return Err(Error::invalid("M1 requires a platform-specific image manifest, not an image index"));}
        let list=String::from_utf8(self.ctr(&strs(&["images","list"])).await?).map_err(|_|Error::unavailable("Invalid image inventory"))?;
        if !list.lines().any(|line|{let x:Vec<_>=line.split_whitespace().collect();x.len()>=3&&x[0]==image&&x[2]==expected}){return Err(Error::not_found());}Ok(())
    }
    pub async fn mount_image(&self,image:&str,sha:&str)->Result<PathBuf>{
        let root=self.config.root().join("images").join(&sha[7..]);fs::create_dir_all(&root)?;
        // ctr mount reuses its view snapshot. Avoid stacking mounts after a restart.
        if !mounted(&root)?{self.ctr(&vec!["images".into(),"mount".into(),image.into(),root.display().to_string()]).await?;}
        if !mounted(&root)?{return Err(Error::unavailable("Image rootfs is not mounted"));}Ok(root)
    }
    fn dir(&self,id:Uuid)->PathBuf{self.config.root().join("runtime").join(id.to_string())}
    pub fn address(&self,id:Uuid)->Result<String>{let v=strict_json(&fs::read(self.dir(id).join("network.json"))?,4096)?;if v["instance"]!=id.to_string(){return Err(Error::conflict("Runtime identity differs"));}v["peer"].as_str().map(str::to_owned).ok_or_else(||Error::unavailable("Missing runtime address"))}
    async fn network(&self,id:Uuid)->Result<Value>{
        let dir=self.dir(id);fs::create_dir_all(&dir)?;let state=dir.join("network.json");
        let short=id.simple().to_string();let host=format!("xh{}",&short[..10]);let peer=format!("xp{}",&short[..10]);let ns=format!("xelab-{id}");
        let v=if state.exists(){strict_json(&fs::read(&state)?,4096)?}else{
            let net:ipnet::Ipv4Net=self.config.text("runtime","network_cidr").parse().map_err(|_|Error::invalid("Invalid network"))?;
            let base=u32::from(net.network());let slots=1u32<<(30-net.prefix_len());let mut used=std::collections::HashSet::new();
            for entry in fs::read_dir(self.config.root().join("runtime"))?{let file=entry?.path().join("network.json");if file.exists(){let x=strict_json(&fs::read(file)?,4096)?;if let Some(slot)=x["slot"].as_u64(){used.insert(slot as u32);}}}
            let slot=(0..slots).find(|n|!used.contains(n)).ok_or_else(Error::exhausted)?;
            let hostip=Ipv4Addr::from(base+slot*4+1).to_string();let peerip=Ipv4Addr::from(base+slot*4+2).to_string();
            let v=json!({"instance":id,"slot":slot,"host":hostip,"peer":peerip});tls::atomic_file(&state,&serde_json::to_vec(&v)?,0o600)?;v
        };
        if v["instance"]!=id.to_string(){return Err(Error::conflict("Network state belongs to another instance"));}
        if Path::new("/run/netns").join(&ns).exists(){return Ok(v);}
        let h=v["host"].as_str().ok_or_else(||Error::invalid("Invalid network state"))?;let p=v["peer"].as_str().ok_or_else(||Error::invalid("Invalid network state"))?;
        h.parse::<Ipv4Addr>().map_err(|_|Error::invalid("Invalid network state"))?;p.parse::<Ipv4Addr>().map_err(|_|Error::invalid("Invalid network state"))?;
        let rpc:std::net::SocketAddr=self.config.text("server","host_rpc_listen").parse().unwrap();
        let nft=format!("table inet xl_{} {{\n chain ingress {{ type filter hook input priority -20; policy accept; iifname \"{host}\" ip daddr {} tcp dport {} accept; iifname \"{host}\" drop; }}\n chain transit {{ type filter hook forward priority -20; policy accept; iifname \"{host}\" drop; oifname \"{host}\" drop; }}\n}}\n",&short[..16],rpc.ip(),rpc.port());
        let nftfile=dir.join("network.nft");tls::atomic_file(&nftfile,nft.as_bytes(),0o600)?;
        command("nft",&vec!["-f".into(),nftfile.display().to_string()],Duration::from_secs(10)).await?;
        command("ip",&strs(&["netns","add",&ns]),Duration::from_secs(10)).await?;
        for args in [vec!["link","add",&host,"type","veth","peer","name",&peer],vec!["link","set",&peer,"netns",&ns],vec!["link","set",&host,"up"]]{command("ip",&strs(&args),Duration::from_secs(10)).await?;}
        command("ip",&strs(&["address","add",&format!("{h}/30"),"dev",&host]),Duration::from_secs(10)).await?;
        for args in [vec!["-n",&ns,"address","add",&format!("{p}/30"),"dev",&peer],vec!["-n",&ns,"link","set","lo","up"],vec!["-n",&ns,"link","set",&peer,"up"]]{command("ip",&strs(&args),Duration::from_secs(10)).await?;}
        if rpc.ip().to_string()!=h{command("ip",&strs(&["-n",&ns,"route","add",&format!("{}/32",rpc.ip()),"via",h,"dev",&peer]),Duration::from_secs(10)).await?;}
        // No default route. Forwarding is denied even if a route is later added.
        Ok(v)
    }
    pub async fn inventory(&self)->Result<Vec<Uuid>>{
        let list=String::from_utf8(self.ctr(&strs(&["containers","list","--quiet"])).await?).map_err(|_|Error::unavailable("Invalid container inventory"))?;
        list.lines().map(|line|line.parse().map_err(|_|Error::conflict("Dedicated namespace contains an unmanaged container"))).collect()
    }
    pub async fn running(&self,id:Uuid)->Result<bool>{
        let list=String::from_utf8(self.ctr(&strs(&["tasks","list"])).await?).map_err(|_|Error::unavailable("Invalid task inventory"))?;
        Ok(list.lines().any(|line|{let fields:Vec<_>=line.split_whitespace().collect();fields.len()>=3&&fields[0]==id.to_string()&&fields[2]=="RUNNING"}))
    }
    pub async fn ensure(&self,id:Uuid,deployment:Uuid,generation:i64,manifest:&Value,image:&str,sha:&str,limits:&Value)->Result<Observation>{
        let dir=self.dir(id);fs::create_dir_all(&dir)?;
        let root=self.mount_image(image,sha).await?;
        let net=self.network(id).await?;
        let identity=dir.join("identity");
        if !identity.join("tls.key").exists(){tls::issue(&self.config,Some((id,generation)),&identity)?;}
        let spec=oci_spec(&self.config,id,deployment,generation,manifest,&root,&identity,limits)?;
        let config=dir.join("config.json");
        if !self.inventory().await?.contains(&id){
            tls::atomic_file(&config,&serde_json::to_vec(&spec)?,0o600)?;
            self.ctr(&vec!["containers".into(),"create".into(),"--config".into(),config.display().to_string(),"--label".into(),format!("org.xelab.instance={id}"),"--label".into(),format!("org.xelab.deployment={deployment}"),"--label".into(),format!("org.xelab.generation={generation}"),id.to_string()]).await?;
        }else{
            let existing=strict_json(&self.ctr(&strs(&["containers","info","--spec",&id.to_string()])).await?,2097152)?;
            if existing["annotations"]["org.xelab.instance"]!=id.to_string()||existing["annotations"]["org.xelab.generation"]!=generation.to_string()||existing["linux"]["cgroupsPath"]!=spec["linux"]["cgroupsPath"]{return Err(Error::conflict("Existing OCI instance identity differs"));}
        }
        if !self.running(id).await?{
            // A stopped task record is removed before recreating the same stable instance.
            let _=self.ctr(&strs(&["tasks","delete",&id.to_string()])).await;
            self.ctr(&strs(&["tasks","start","--detach",&id.to_string()])).await?;
        }
        let cgroup=Path::new("/sys/fs/cgroup/xelab/plugins").join(id.to_string());
        let expected=format!("{} 100000",limits["cpu_millis"].as_i64().unwrap()*100);
        if fs::read_to_string(cgroup.join("cpu.max"))?.trim()!=expected||fs::read_to_string(cgroup.join("memory.max"))?.trim()!=limits["memory_bytes"].to_string(){return Err(Error::unavailable("Runtime did not enforce resource limits"));}
        Ok(Observation{running:self.running(id).await?,address:net["peer"].as_str().unwrap().to_owned()})
    }
    pub async fn stop(&self,id:Uuid)->Result<()> {
        if self.running(id).await?{self.ctr(&strs(&["tasks","kill","--signal","SIGKILL","--all",&id.to_string()])).await?;}
        for _ in 0..40{if !self.running(id).await?{break;}tokio::time::sleep(Duration::from_millis(50)).await;}
        if self.running(id).await?{return Err(Error::unavailable("Runtime task has not stopped"));}
        if self.inventory().await?.contains(&id){self.ctr(&strs(&["containers","delete",&id.to_string()])).await?;}
        let cg=Path::new("/sys/fs/cgroup/xelab/plugins").join(id.to_string());
        if cg.exists()&&!fs::read_to_string(cg.join("cgroup.procs"))?.trim().is_empty(){return Err(Error::unavailable("Runtime cgroup still contains processes"));}
        let short=id.simple().to_string();let ns=format!("xelab-{id}");
        if Path::new("/run/netns").join(&ns).exists(){command("ip",&strs(&["netns","delete",&ns]),Duration::from_secs(10)).await?;}
        let host=format!("xh{}",&short[..10]);
        if Path::new("/sys/class/net").join(&host).exists(){command("ip",&strs(&["link","delete",&host]),Duration::from_secs(10)).await?;}
        let _=command("nft",&strs(&["delete","table","inet",&format!("xl_{}",&short[..16])]),Duration::from_secs(10)).await;
        if self.dir(id).exists(){fs::remove_dir_all(self.dir(id))?;}Ok(())
    }
}

fn mounted(path:&Path)->Result<bool>{let target=path.to_str().ok_or_else(||Error::invalid("Invalid runtime path"))?;Ok(fs::read_to_string("/proc/self/mountinfo")?.lines().any(|l|l.split_whitespace().nth(4)==Some(target)))}

pub fn oci_spec(config:&Config,id:Uuid,deployment:Uuid,generation:i64,manifest:&Value,root:&Path,identity:&Path,limits:&Value)->Result<Value>{
    crate::contracts::validate("ResourceLimits",limits)?;
    let env=vec!["PATH=/usr/local/bin:/usr/bin:/bin".to_owned(),format!("XELAB_PLUGIN_ID={}",manifest["plugin_id"].as_str().unwrap()),format!("XELAB_PLUGIN_VERSION={}",manifest["version"].as_str().unwrap()),format!("XELAB_DEPLOYMENT_ID={deployment}"),format!("XELAB_INSTANCE_ID={id}"),format!("XELAB_GENERATION={generation}"),format!("XELAB_HOST_RPC_ENDPOINT=https://{}",config.text("server","host_rpc_listen")),"XELAB_TLS_CERT_PATH=/run/xelab/identity/tls.crt".into(),"XELAB_TLS_KEY_PATH=/run/xelab/identity/tls.key".into(),"XELAB_TLS_CA_PATH=/run/xelab/identity/ca.crt".into(),"XELAB_PLUGIN_RPC_LISTEN=0.0.0.0:7000".into()];
    let tmp=format!("size={},mode=1777",limits["ephemeral_bytes"]);
    Ok(json!({"ociVersion":"1.0.2","hostname":"xelab-plugin","root":{"path":root,"readonly":true},"process":{"terminal":false,"user":{"uid":65532,"gid":65532},"args":manifest["entrypoint"],"env":env,"cwd":"/","noNewPrivileges":true,"capabilities":{"bounding":[],"effective":[],"inheritable":[],"permitted":[],"ambient":[]},"rlimits":[{"type":"RLIMIT_NOFILE","hard":1024,"soft":1024}]},"mounts":[{"destination":"/proc","type":"proc","source":"proc","options":["nosuid","noexec","nodev"]},{"destination":"/dev","type":"tmpfs","source":"tmpfs","options":["nosuid","strictatime","mode=755","size=65536k"]},{"destination":"/tmp","type":"tmpfs","source":"tmpfs","options":["nosuid","nodev","noexec",tmp]},{"destination":"/run/xelab/identity","type":"bind","source":identity,"options":["rbind","ro","nosuid","nodev","noexec"]}],"annotations":{"org.xelab.instance":id.to_string(),"org.xelab.deployment":deployment.to_string(),"org.xelab.generation":generation.to_string()},"linux":{"cgroupsPath":format!("/xelab/plugins/{id}"),"namespaces":[{"type":"pid"},{"type":"ipc"},{"type":"uts"},{"type":"mount"},{"type":"cgroup"},{"type":"network","path":format!("/run/netns/xelab-{id}")}],"resources":{"cpu":{"period":100000,"quota":limits["cpu_millis"].as_u64().unwrap()*100},"memory":{"limit":limits["memory_bytes"],"swap":limits["memory_bytes"]},"pids":{"limit":limits["pids"]},"devices":[{"allow":false,"access":"rwm"},{"allow":true,"type":"c","major":1,"minor":3,"access":"rw"},{"allow":true,"type":"c","major":1,"minor":5,"access":"rw"},{"allow":true,"type":"c","major":1,"minor":8,"access":"rw"},{"allow":true,"type":"c","major":1,"minor":9,"access":"rw"}]},"devices":[{"path":"/dev/null","type":"c","major":1,"minor":3,"fileMode":438,"uid":0,"gid":0},{"path":"/dev/zero","type":"c","major":1,"minor":5,"fileMode":438,"uid":0,"gid":0},{"path":"/dev/random","type":"c","major":1,"minor":8,"fileMode":438,"uid":0,"gid":0},{"path":"/dev/urandom","type":"c","major":1,"minor":9,"fileMode":438,"uid":0,"gid":0}],"maskedPaths":["/proc/kcore","/proc/keys","/proc/timer_list","/proc/sched_debug","/sys/firmware"],"readonlyPaths":["/proc/asound","/proc/bus","/proc/fs","/proc/irq","/proc/sys","/proc/sysrq-trigger"],"seccomp":{"defaultAction":"SCMP_ACT_ALLOW","syscalls":[{"names":["mount","umount2","pivot_root","ptrace","process_vm_readv","process_vm_writev","bpf","perf_event_open","kexec_load","kexec_file_load","init_module","finit_module","delete_module","reboot","swapon","swapoff","open_by_handle_at","name_to_handle_at","setns","unshare","keyctl","add_key","request_key","userfaultfd","io_uring_setup"],"action":"SCMP_ACT_ERRNO","errnoRet":1}]}}}))
}

#[cfg(test)]mod tests{use super::*;
    #[test]fn oci_spec_is_nonprivileged_and_has_hard_limits(){let t:toml::Value=toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();let c=Config::from_value(serde_json::to_value(t).unwrap()).unwrap();let limits=json!({"cpu_millis":500,"memory_bytes":134217728,"ephemeral_bytes":1048576,"pids":32});let spec=oci_spec(&c,Uuid::new_v4(),Uuid::new_v4(),1,&json!({"plugin_id":"org.xelab.echo","version":"1.0.0","entrypoint":["/echo"]}),Path::new("/trusted/rootfs"),Path::new("/trusted/identity"),&limits).unwrap();assert_eq!(spec["process"]["user"]["uid"],65532);assert_eq!(spec["root"]["readonly"],true);assert_eq!(spec["linux"]["resources"]["cpu"]["quota"],50000);assert_eq!(spec["process"]["capabilities"]["effective"],json!([]));assert!(spec["mounts"].as_array().unwrap().iter().all(|m|m["source"]!="/"));}
}
