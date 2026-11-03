//! A restricted OCI reader, never an extractor. Reject unknown tar formats,
//! links and paths; hash every blob and validate the referenced manifest graph.
use super::*;
use serde_json::Value;
const MAX_ENTRIES: usize = 65536;
const JSON_LIMIT: usize = 2 * 1024 * 1024;
const JSON_TOTAL: usize = 16 * 1024 * 1024;
fn octal(bytes: &[u8]) -> Result<u64> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::invalid("Invalid OCI tar number"))?
        .trim_matches(['\0', ' ']);
    if text.is_empty() || !text.bytes().all(|c| (b'0'..=b'7').contains(&c)) {
        return Err(Error::invalid("Unsupported OCI tar number"));
    }
    u64::from_str_radix(text, 8).map_err(|_| Error::invalid("OCI tar number exceeds its limit"))
}
fn digest(value: &Value) -> Result<&str> {
    let id = value["digest"]
        .as_str()
        .ok_or_else(|| Error::invalid("OCI descriptor has no digest"))?;
    if id.len() != 71
        || !id.starts_with("sha256:")
        || !id[7..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(Error::invalid("Unsupported OCI descriptor digest"));
    }
    Ok(id)
}
fn descriptor(value: &Value, blobs: &BTreeMap<String, u64>) -> Result<()> {
    if value
        .get("urls")
        .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
        || value.get("data").is_some()
    {
        return Err(Error::invalid(
            "External or embedded OCI content is unsupported",
        ));
    }
    let id = digest(value)?;
    let size = value["size"]
        .as_u64()
        .ok_or_else(|| Error::invalid("OCI descriptor has no size"))?;
    if blobs.get(id) != Some(&size) {
        return Err(Error::invalid(
            "OCI archive is missing verified descriptor content",
        ));
    }
    Ok(())
}

pub(super) async fn verify(
    input: &mut (impl AsyncRead + Unpin),
    images: &[String],
    maximum: u64,
) -> Result<()> {
    let expected: BTreeSet<String> = images
        .iter()
        .map(|s| artifact::image_ref(s).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut blobs = BTreeMap::new();
    let mut manifests = BTreeMap::new();
    let mut index = None;
    let mut layout = false;
    let mut json_bytes = 0usize;
    let mut consumed = 0u64;
    let mut count = 0usize;
    loop {
        let mut header = [0u8; 512];
        input.read_exact(&mut header).await?;
        consumed += 512;
        if consumed > maximum {
            return Err(Error::exhausted());
        }
        if header.iter().all(|b| *b == 0) {
            let mut end = [0; 512];
            input.read_exact(&mut end).await?;
            consumed += 512;
            if end.iter().any(|b| *b != 0) {
                return Err(Error::invalid("OCI archive has an invalid tar end"));
            }
            let mut padding = [0; BUFFER];
            loop {
                let n = input.read(&mut padding).await?;
                if n == 0 {
                    break;
                }
                consumed += n as u64;
                if consumed > maximum || padding[..n].iter().any(|b| *b != 0) {
                    return Err(Error::invalid("OCI archive has trailing data"));
                }
            }
            if consumed != maximum || !consumed.is_multiple_of(512) {
                return Err(Error::invalid("OCI tar length differs"));
            }
            break;
        }
        count += 1;
        let checksum: u64 = header
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    *b as u64
                }
            })
            .sum();
        if count > MAX_ENTRIES
            || checksum != octal(&header[148..156])?
            || &header[257..263] != b"ustar\0"
            || header[345..500].iter().any(|b| *b != 0)
            || header[157..257].iter().any(|b| *b != 0)
        {
            return Err(Error::invalid("Unsupported or damaged OCI tar header"));
        }
        let end = header[..100].iter().position(|b| *b == 0).unwrap_or(100);
        if header[end..100].iter().any(|b| *b != 0) {
            return Err(Error::invalid("OCI tar name is malformed"));
        }
        let name = std::str::from_utf8(&header[..end])
            .map_err(|_| Error::invalid("OCI tar name is invalid"))?;
        let size = octal(&header[124..136])?;
        if consumed.checked_add(size).is_none_or(|n| n > maximum) {
            return Err(Error::exhausted());
        }
        if header[156] == b'5' && matches!(name, "blobs/" | "blobs/sha256/") && size == 0 {
            continue;
        }
        if !matches!(header[156], 0 | b'0') {
            return Err(Error::invalid(
                "OCI tar links and special entries are unsupported",
            ));
        }
        let blob = name.strip_prefix("blobs/sha256/");
        if !matches!(name, "oci-layout" | "index.json")
            && !blob.is_some_and(|b| {
                b.len() == 64
                    && b.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
        {
            return Err(Error::invalid("OCI tar contains an unmanaged path"));
        }
        let id = blob.map(|b| format!("sha256:{b}"));
        let capture = id.as_ref().is_some_and(|id| expected.contains(id)) || blob.is_none();
        if capture
            && (size > JSON_LIMIT as u64
                || json_bytes
                    .checked_add(size as usize)
                    .is_none_or(|n| n > JSON_TOTAL))
        {
            return Err(Error::exhausted());
        }
        let mut bytes = Vec::new();
        let mut hash = Sha256::new();
        let mut left = size;
        let mut buffer = [0; BUFFER];
        while left > 0 {
            let n = left.min(BUFFER as u64) as usize;
            input.read_exact(&mut buffer[..n]).await?;
            hash.update(&buffer[..n]);
            if capture {
                bytes.extend_from_slice(&buffer[..n]);
            }
            left -= n as u64;
            consumed += n as u64;
        }
        if let Some(id) = id {
            if format!("sha256:{:x}", hash.finalize()) != id
                || blobs.insert(id.clone(), size).is_some()
            {
                return Err(Error::invalid("OCI blob digest differs or repeats"));
            }
            if capture {
                json_bytes += bytes.len();
                manifests.insert(id, bytes);
            }
        } else {
            json_bytes += bytes.len();
            let value = strict_json(&bytes, JSON_LIMIT)?;
            if name == "oci-layout" {
                if layout || value != serde_json::json!({"imageLayoutVersion":"1.0.0"}) {
                    return Err(Error::invalid("OCI layout is unsupported or repeated"));
                }
                layout = true;
            } else if index.replace(value).is_some() {
                return Err(Error::invalid("OCI index repeats"));
            }
        }
        let padding = (512 - size % 512) % 512;
        if padding > 0 {
            let mut block = [0u8; 512];
            input.read_exact(&mut block[..padding as usize]).await?;
            consumed += padding;
            if block[..padding as usize].iter().any(|b| *b != 0) {
                return Err(Error::invalid("OCI entry padding is invalid"));
            }
        }
    }
    let index = index.ok_or_else(|| Error::invalid("OCI archive has no index"))?;
    let roots = index["manifests"]
        .as_array()
        .ok_or_else(|| Error::invalid("OCI archive index is invalid"))?;
    if !layout || index["schemaVersion"] != 2 || roots.is_empty() || roots.len() > MAX_IMAGES {
        return Err(Error::invalid("OCI archive layout is invalid"));
    }
    let mut found = BTreeSet::new();
    for root in roots {
        descriptor(root, &blobs)?;
        let id = digest(root)?;
        if !expected.contains(id) {
            return Err(Error::invalid("OCI index includes an unexpected image"));
        }
        found.insert(id.to_owned());
    }
    if found != expected {
        return Err(Error::invalid(
            "OCI index does not cover every requested image",
        ));
    }
    for id in expected {
        let data = manifests
            .get(&id)
            .ok_or_else(|| Error::invalid("OCI image manifest is absent"))?;
        let manifest = strict_json(data, JSON_LIMIT)?;
        if manifest["schemaVersion"] != 2
            || !matches!(
                manifest["mediaType"].as_str(),
                Some(
                    "application/vnd.oci.image.manifest.v1+json"
                        | "application/vnd.docker.distribution.manifest.v2+json"
                )
            )
        {
            return Err(Error::invalid("OCI image manifest is unsupported"));
        }
        descriptor(&manifest["config"], &blobs)?;
        let layers = manifest["layers"]
            .as_array()
            .ok_or_else(|| Error::invalid("OCI image layers are invalid"))?;
        if layers.len() > 4096 {
            return Err(Error::exhausted());
        }
        for layer in layers {
            descriptor(layer, &blobs)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn entry(tar: &mut Vec<u8>, name: &str, data: &[u8], kind: u8) {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000600\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        header[148..156].fill(b' ');
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u32 = header.iter().map(|b| *b as u32).sum();
        header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
        tar.extend(header);
        tar.extend(data);
        tar.resize(tar.len() + ((512 - data.len() % 512) % 512), 0);
    }
    fn blob(tar: &mut Vec<u8>, data: &[u8]) -> Value {
        let hash = format!("{:x}", Sha256::digest(data));
        entry(tar, &format!("blobs/sha256/{hash}"), data, b'0');
        json!({"digest":format!("sha256:{hash}"),"size":data.len()})
    }
    fn fixture(missing: bool, wrong_size: bool) -> (Vec<u8>, Vec<String>) {
        let mut tar = Vec::new();
        entry(
            &mut tar,
            "oci-layout",
            br#"{"imageLayoutVersion":"1.0.0"}"#,
            b'0',
        );
        let config = if missing {
            json!({"digest":format!("sha256:{}","f".repeat(64)),"size":2})
        } else {
            blob(&mut tar, b"{}")
        };
        let mut layer = blob(&mut tar, b"immutable layer contents");
        if wrong_size {
            layer["size"] = json!(9999);
        }
        let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":config,"layers":[layer]})).unwrap();
        let root = blob(&mut tar, &manifest);
        let image = format!("example.test/archive@{}", root["digest"].as_str().unwrap());
        entry(
            &mut tar,
            "index.json",
            &serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[root]})).unwrap(),
            b'0',
        );
        tar.resize(tar.len() + 1024, 0);
        (tar, vec![image])
    }
    #[tokio::test]
    async fn actual_tar_bytes_require_all_digests_and_the_full_image_graph() {
        let (tar, images) = fixture(false, false);
        verify(&mut tar.as_slice(), &images, tar.len() as u64)
            .await
            .unwrap();
        for (missing, wrong_size) in [(true, false), (false, true)] {
            let (tar, images) = fixture(missing, wrong_size);
            assert!(verify(&mut tar.as_slice(), &images, tar.len() as u64)
                .await
                .is_err());
        }
        let mut corrupt = tar.clone();
        let offset = corrupt
            .windows(24)
            .position(|w| w == b"immutable layer contents")
            .unwrap();
        corrupt[offset] ^= 1;
        assert!(
            verify(&mut corrupt.as_slice(), &images, corrupt.len() as u64)
                .await
                .is_err()
        );
        assert!(verify(
            &mut &tar[..tar.len() - 512],
            &images,
            (tar.len() - 512) as u64
        )
        .await
        .is_err());
        let mut trailing = tar.clone();
        trailing.extend([1u8; 512]);
        assert!(
            verify(&mut trailing.as_slice(), &images, trailing.len() as u64)
                .await
                .is_err()
        );
        for (name, kind) in [("../escape", b'0'), ("blobs/sha256/link", b'2')] {
            let mut invalid = Vec::new();
            entry(&mut invalid, name, b"bad", kind);
            invalid.resize(invalid.len() + 1024, 0);
            assert!(
                verify(&mut invalid.as_slice(), &images, invalid.len() as u64)
                    .await
                    .is_err()
            );
        }
    }
}
