use crate::{contracts::{digest,id,strict_json,string,validate,validate_business},error::{Error,Result},runtime::Runtime};
use base64::{Engine,engine::general_purpose::STANDARD};
use ed25519_dalek::{pkcs8::DecodePublicKey,Signature,Verifier,VerifyingKey};
use serde_json::Value;
use std::{fs::OpenOptions,io::Read,os::unix::fs::OpenOptionsExt,path::{Path,Component}};

pub struct Artifact{pub image:String,pub manifest:Value,pub raw:Vec<u8>,pub image_digest:String,pub manifest_digest:String}

pub fn image_ref(image:&str)->Result<&str>{
    if image.len()>2048||!image.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)||!image.bytes().all(|c|c.is_ascii_alphanumeric()||b"/._-:@".contains(&c)){return Err(Error::invalid("Invalid immutable OCI reference"));}
    let(_,d)=image.rsplit_once('@').ok_or_else(||Error::invalid("OCI digest is required"))?;
    if d.len()!=71||!d.starts_with("sha256:")||!d[7..].bytes().all(|c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c)){return Err(Error::invalid("Invalid SHA-256 image digest"));}Ok(d)
}

pub fn read_image_file(root:&Path,relative:&str,maximum:usize)->Result<Vec<u8>>{
    let mut path=root.to_owned();
    let parts:Vec<_>=Path::new(relative).components().collect();
    if parts.is_empty(){return Err(Error::invalid("Invalid artifact path"));}
    for part in parts{
        let Component::Normal(name)=part else{return Err(Error::invalid("Artifact path escapes its root"));};path.push(name);
        if path.symlink_metadata()?.file_type().is_symlink(){return Err(Error::invalid("Signed artifact paths cannot contain symbolic links"));}
    }
    let f=OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)?;
    if !f.metadata()?.is_file(){return Err(Error::invalid("Artifact member must be a regular file"));}
    let mut bytes=Vec::new();f.take(maximum as u64+1).read_to_end(&mut bytes)?;
    if bytes.len()>maximum{return Err(Error::invalid("Artifact member exceeds its size limit"));}Ok(bytes)
}

pub fn signature_envelope(manifest:&Value,image_digest:&str,raw:&[u8])->Result<String>{
    let publisher=id(manifest,"publisher_id")?;let plugin=string(manifest,"plugin_id")?;let version=string(manifest,"version")?;
    if [plugin,version,image_digest].iter().any(|s|s.contains(['\r','\n'])){return Err(Error::invalid("Invalid signature field"));}
    Ok(format!("xelab-artifact-v1\n{publisher}\n{plugin}\n{version}\n{image_digest}\n{}\n",digest(raw)))
}

pub fn verify_signature(key:&str,signature:&str,manifest:&Value,image_digest:&str,raw:&[u8])->Result<()> {
    let key=VerifyingKey::from_public_key_pem(key).map_err(|_|Error::invalid("Expected an Ed25519 public key"))?;
    let bytes=STANDARD.decode(signature).map_err(|_|Error::invalid("Invalid signature encoding"))?;
    let sig=Signature::from_slice(&bytes).map_err(|_|Error::invalid("Invalid Ed25519 signature"))?;
    key.verify(signature_envelope(manifest,image_digest,raw)?.as_bytes(),&sig).map_err(|_|Error::invalid("Artifact signature verification failed"))
}

pub fn validate_profile(manifest:&Value)->Result<()> {
    validate("Manifest",manifest)?;
    for field in ["dependencies","requested_rules","volumes","routes","event_topics","skills","extension_points"] {
        if manifest.get(field).is_some_and(|v|!v.as_array().is_some_and(Vec::is_empty)){return Err(Error::invalid("This M1 profile supports only stateless ordinary plugins without dependencies, proxy rules, UI, volumes or extensions"));}
    }
    if manifest["data_schema_version"]!="none"||manifest.get("business_entry").is_some_and(|v|v!="direct")||manifest.get("webui").is_some()||manifest.get("ui").is_some()||manifest.get("micro_runtime").is_some()||manifest.get("extension_host").is_some(){return Err(Error::invalid("Unsupported plugin profile"));}
    let platform=if cfg!(target_arch="x86_64"){"linux/amd64"}else if cfg!(target_arch="aarch64"){"linux/arm64"}else{return Err(Error::invalid("Unsupported host architecture"));};
    if !manifest["platforms"].as_array().unwrap().iter().any(|v|v==platform){return Err(Error::invalid("Artifact does not support this host platform"));}
    crate::contracts::reject_refs(&manifest["config_schema"])?;
    for cap in manifest["capabilities"].as_array().unwrap(){
        if cap["side_effect"]!="none"||!cap["stream_item_schema"].is_null(){return Err(Error::invalid("M1 accepts only non-streaming side_effect=none capabilities"));}
        for key in ["input_schema","output_schema"]{crate::contracts::reject_refs(&cap[key])?;}
    }Ok(())
}

impl Artifact{
    pub async fn inspect(runtime:&Runtime,request:&Value,key:&str)->Result<Self>{
        validate("InstallRequest",request)?;let image=string(request,"image")?;let image_digest=image_ref(image)?.to_owned();
        // Host administrators import images into the dedicated namespace. No unbounded registry pull runs in the daemon.
        runtime.verify_image(image,&image_digest).await?;
        let root=runtime.mount_image(image,&image_digest).await?;
        let raw=read_image_file(&root,"xelab/plugin.json",1048576)?;let manifest=strict_json(&raw,1048576)?;
        if manifest["publisher_id"]!=request["publisher_id"]{return Err(Error::invalid("Publisher does not match the signed manifest"));}
        verify_signature(key,string(request,"signature")?,&manifest,&image_digest,&raw)?;
        validate_profile(&manifest)?;
        for cap in manifest["capabilities"].as_array().unwrap(){
            let name=string(&cap["contract"],"name")?;let version=string(&cap["contract"],"version")?;
            let mut checks=String::new();
            for (file,field)in[("input.schema.json","input_schema"),("output.schema.json","output_schema"),("stream.schema.json","stream_item_schema"),("semantics.json","")]{
                let bytes=read_image_file(&root,&format!("xelab/contracts/{name}/{version}/{file}"),1048576)?;
                let value=strict_json(&bytes,1048576)?;
                if !field.is_empty()&&value!=cap[field]{return Err(Error::invalid("Contract package differs from signed manifest"));}
                if field.is_empty()&&value["side_effect"]!=cap["side_effect"]{return Err(Error::invalid("Contract side effects differ"));}
                checks.push_str(&format!("{file} {}\n",&digest(&bytes)[7..]));
            }
            if digest(checks.as_bytes())!=cap["contract"]["schema_digest"]{return Err(Error::invalid("Contract package digest differs"));}
        }
        Ok(Self{image:image.to_owned(),manifest_digest:digest(&raw),manifest,raw,image_digest})
    }
    pub fn validate_deployment(&self,spec:&Value)->Result<()> {
        validate("DeploymentSpec",spec)?;
        if spec["replicas"].as_u64().unwrap_or(1)!=1||!spec["volume_ids"].as_array().unwrap().is_empty(){return Err(Error::invalid("M1 requires one stateless replica without volumes"));}
        for field in ["cpu_millis","memory_bytes","ephemeral_bytes","pids"]{if spec["limits"][field].as_u64()<self.manifest["minimum_resources"][field].as_u64(){return Err(Error::invalid("Deployment limits are below the signed minimum"));}}
        validate_business(&self.manifest["config_schema"],&spec["config"])
    }
}

#[cfg(test)]mod tests{use super::*;use serde_json::json;
    #[test]fn only_fixed_safe_image_names_are_accepted(){assert!(image_ref(&format!("example.org/a@sha256:{}","a".repeat(64))).is_ok());for s in ["example.org/a:latest","--help","a,b@sha256:1","a@sha256:ABC"]{assert!(image_ref(s).is_err());}}
    #[test]fn envelope_preserves_raw_manifest_bytes_and_final_newline(){let m=json!({"publisher_id":uuid::Uuid::nil(),"plugin_id":"org.xelab.echo","version":"1.0.0"});let e=signature_envelope(&m,"sha256:abc",b"{}\n").unwrap();assert!(e.ends_with('\n'));assert!(e.contains(&digest(b"{}\n")));assert!(!e.contains(&digest(b"{}")));}
    #[test]fn image_reader_rejects_symlink_escape(){let d=tempfile::tempdir().unwrap();std::os::unix::fs::symlink("/etc",d.path().join("xelab")).unwrap();assert!(read_image_file(d.path(),"xelab/passwd",1024).is_err());assert!(read_image_file(d.path(),"../passwd",1024).is_err());}
}
