use crate::{
    contracts::{openapi, validate},
    error::{Error, Result},
};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::Read,
    net::SocketAddr,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Config {
    pub value: Value,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path)?;
        if source.len() > 65536 {
            return Err(Error::invalid("Configuration is too large"));
        }
        let t: toml::Value =
            toml::from_str(&source).map_err(|_| Error::invalid("Invalid TOML configuration"))?;
        Self::from_value(serde_json::to_value(t)?)
    }
    pub fn from_value(value: Value) -> Result<Self> {
        let raw = include_str!("../../../docs/contracts/kernel-config.schema.json").replace(
            "./openapi.json#/components/schemas/",
            "#/components/schemas/",
        );
        let mut schema: Value = serde_json::from_str(&raw)?;
        schema["components"] = openapi()["components"].clone();
        let compiled = jsonschema::JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .should_validate_formats(true)
            .compile(&schema)
            .map_err(|_| Error::invalid("Invalid configuration schema"))?;
        if !compiled.is_valid(&value) {
            return Err(Error::invalid(
                "Configuration does not match kernel-config.schema.json",
            ));
        }
        validate("NodeConfig", &value["resources"])?;
        validate("QuotaSpec", &value["quota_defaults"])?;
        let c = Self { value };
        c.origin()?;
        c.listen()?;
        let _: SocketAddr = c
            .text("server", "host_rpc_listen")
            .parse()
            .map_err(|_| Error::invalid("Host RPC address must be an IP address and port"))?;
        let net: ipnet::Ipv4Net = c
            .text("runtime", "network_cidr")
            .parse()
            .map_err(|_| Error::invalid("M1 requires a dedicated IPv4 CIDR"))?;
        if !(16..=28).contains(&net.prefix_len()) {
            return Err(Error::invalid(
                "M1 plugin CIDR prefix must be /16 through /28",
            ));
        }
        for section in ["server", "database", "runtime", "storage", "security"] {
            for (key, v) in c.value[section].as_object().unwrap() {
                if key.ends_with("_file")
                    || ["root_path", "socket", "unix_socket"].contains(&key.as_str())
                {
                    let p = Path::new(v.as_str().unwrap());
                    if !p.is_absolute()
                        || p.components()
                            .any(|x| matches!(x, std::path::Component::ParentDir))
                    {
                        return Err(Error::invalid("Host paths must be absolute and normalized"));
                    }
                }
            }
        }
        if !c.value["ui_gateway"].is_null() {
            let base = c.text("ui_gateway", "base_domain");
            if base.parse::<std::net::IpAddr>().is_ok()
                || base
                    .split('.')
                    .any(|s| s.is_empty() || s.len() > 63 || s.starts_with('-') || s.ends_with('-'))
            {
                return Err(Error::invalid("Invalid UI base domain"));
            }
            for s in c.value["ui_gateway"]["shell_origins"].as_array().unwrap() {
                let origin = checked_origin(s.as_str().unwrap())?;
                if origin == c.origin()?
                    || origin
                        .host_str()
                        .is_some_and(|h| h == base || h.ends_with(&format!(".{base}")))
                {
                    return Err(Error::invalid("UI origins must be isolated"));
                }
            }
        }
        Ok(c)
    }
    pub fn text(&self, section: &str, key: &str) -> &str {
        self.value[section][key].as_str().expect("validated config")
    }
    pub fn root(&self) -> PathBuf {
        PathBuf::from(self.text("storage", "root_path"))
    }
    pub fn origin(&self) -> Result<url::Url> {
        checked_origin(self.text("server", "public_origin"))
    }
    pub fn listen(&self) -> Result<SocketAddr> {
        self.text("server", "listen")
            .parse()
            .map_err(|_| Error::invalid("Listen address must be an IP address and port"))
    }
    pub fn secrets(&self) -> Result<(String, Vec<u8>)> {
        let dsn = String::from_utf8(read_private(
            Path::new(self.text("database", "dsn_file")),
            16384,
        )?)
        .map_err(|_| Error::invalid("Invalid DSN encoding"))?;
        let key = read_private(
            Path::new(self.text("security", "token_hmac_key_file")),
            4096,
        )?;
        if key.len() < 32 {
            return Err(Error::invalid(
                "Token HMAC key must contain at least 32 bytes",
            ));
        }
        if read_private(Path::new(self.text("security", "master_key_file")), 32)?.len() != 32 {
            return Err(Error::invalid("Master key must contain exactly 32 bytes"));
        }
        read_private(Path::new(self.text("server", "tls_key_file")), 16384)?;
        read_private(
            Path::new(self.text("security", "workload_ca_key_file")),
            16384,
        )?;
        Ok((dsn.trim().to_owned(), key))
    }
}

pub fn checked_origin(s: &str) -> Result<url::Url> {
    let u = url::Url::parse(s).map_err(|_| Error::invalid("Invalid HTTPS origin"))?;
    if u.scheme() != "https"
        || u.host_str().is_none()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(Error::invalid("Expected an exact HTTPS origin"));
    }
    Ok(u)
}

pub fn read_private(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let m = f.metadata()?;
    if !m.is_file()
        || m.mode() & 0o777 != 0o600
        || m.uid() != unsafe { libc::geteuid() }
        || m.nlink() != 1
    {
        return Err(Error::invalid(
            "Private files require owner-only mode 0600, one link and the current owner",
        ));
    }
    let mut bytes = Vec::new();
    f.take(maximum as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(Error::invalid("Private file exceeds its limit"));
    }
    Ok(bytes)
}

pub fn require_host_admin() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        Err(Error::denied())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn validates_repository_example_without_reading_secrets() {
        let t: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        assert!(Config::from_value(serde_json::to_value(t).unwrap()).is_ok());
    }
    #[test]
    fn unsafe_origins_and_private_files_are_rejected() {
        for u in [
            "http://x.test",
            "https://user@x.test",
            "https://x.test/a",
            "https://x.test/?a=1",
        ] {
            assert!(checked_origin(u).is_err());
        }
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("key");
        fs::write(&p, b"secret").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_private(&p, 32).is_err());
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_private(&p, 32).is_ok());
        let sy = d.path().join("symlink");
        std::os::unix::fs::symlink(&p, &sy).unwrap();
        assert!(read_private(&sy, 32).is_err());
    }
}
