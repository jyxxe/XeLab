//! Versioned streaming backup encryption. Every object derives a separate key;
//! authenticated records bind order, object identity and a mandatory final hash.
//! Open only into an unpublished destination: truncation can fail after earlier
//! records were individually authenticated and written there.
use crate::error::{Error, Result};
use ring::{
    aead, hkdf,
    rand::{SecureRandom, SystemRandom},
};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use uuid::Uuid;
use zeroize::Zeroizing;
const MAGIC: &[u8; 8] = b"XELABBK1";
const HEADER: usize = 88;
const CHUNK: usize = 65536;
const FINAL: u32 = 1 << 31;
const MAX_RECORDS: u64 = u32::MAX as u64;
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub bytes: u64,
    pub digest: String,
}
fn integrity() -> Error {
    Error::unavailable("Backup integrity verification failed")
}
fn key(master: &[u8; 32], header: &[u8; HEADER]) -> Result<aead::LessSafeKey> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, &header[72..88]);
    let prk = salt.extract(master);
    let context = [b"xelab/backup/object/v1".as_slice(), &header[8..40]];
    let material = prk
        .expand(&context, &aead::AES_256_GCM)
        .map_err(|_| integrity())?;
    let mut bytes = Zeroizing::new([0u8; 32]);
    material.fill(bytes.as_mut()).map_err(|_| integrity())?;
    Ok(aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, bytes.as_ref()).map_err(|_| integrity())?,
    ))
}
fn nonce(sequence: u64) -> Result<aead::Nonce> {
    if sequence >= MAX_RECORDS {
        return Err(Error::exhausted());
    }
    let mut bytes = [0u8; 12];
    bytes[4..].copy_from_slice(&sequence.to_be_bytes());
    Ok(aead::Nonce::assume_unique_for_key(bytes))
}
fn aad(header_hash: &[u8], sequence: u64, record: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(44);
    out.extend_from_slice(header_hash);
    out.extend_from_slice(&sequence.to_be_bytes());
    out.extend_from_slice(&record.to_be_bytes());
    out
}
fn record(
    w: &mut impl Write,
    key: &aead::LessSafeKey,
    header_hash: &[u8],
    sequence: u64,
    tag: u32,
    plaintext: &[u8],
) -> Result<()> {
    let mut bytes = Zeroizing::new(plaintext.to_vec());
    key.seal_in_place_append_tag(
        nonce(sequence)?,
        aead::Aad::from(aad(header_hash, sequence, tag)),
        &mut *bytes,
    )
    .map_err(|_| integrity())?;
    w.write_all(&tag.to_be_bytes())?;
    w.write_all(&bytes)?;
    Ok(())
}
pub fn seal(
    mut input: impl Read,
    mut output: impl Write,
    master: &[u8; 32],
    backup: Uuid,
    object: Uuid,
    maximum: u64,
) -> Result<Summary> {
    let mut header = [0u8; HEADER];
    header[..8].copy_from_slice(MAGIC);
    header[8..24].copy_from_slice(backup.as_bytes());
    header[24..40].copy_from_slice(object.as_bytes());
    header[40..72].copy_from_slice(&Sha256::digest(master));
    SystemRandom::new()
        .fill(&mut header[72..])
        .map_err(|_| integrity())?;
    let cipher = key(master, &header)?;
    let header_hash = Sha256::digest(header);
    output.write_all(&header)?;
    let mut buffer = Zeroizing::new(vec![0u8; CHUNK]);
    let mut size = 0u64;
    let mut hash = Sha256::new();
    let mut sequence = 0u64;
    loop {
        let mut length = 0;
        while length < CHUNK {
            match input.read(&mut buffer[length..]) {
                Ok(0) => break,
                Ok(n) => length += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        if length == 0 {
            break;
        }
        size = size
            .checked_add(length as u64)
            .filter(|n| *n <= maximum)
            .ok_or_else(Error::exhausted)?;
        hash.update(&buffer[..length]);
        record(
            &mut output,
            &cipher,
            &header_hash,
            sequence,
            length as u32,
            &buffer[..length],
        )?;
        sequence += 1;
    }
    let digest = hash.finalize();
    let mut footer = [0u8; 40];
    footer[..8].copy_from_slice(&size.to_be_bytes());
    footer[8..].copy_from_slice(&digest);
    record(
        &mut output,
        &cipher,
        &header_hash,
        sequence,
        FINAL | 40,
        &footer,
    )?;
    output.flush()?;
    Ok(Summary {
        bytes: size,
        digest: format!("sha256:{digest:x}"),
    })
}
pub fn open(
    mut input: impl Read,
    mut output: impl Write,
    master: &[u8; 32],
    backup: Uuid,
    object: Uuid,
    maximum: u64,
) -> Result<Summary> {
    let mut header = [0u8; HEADER];
    input.read_exact(&mut header).map_err(|_| integrity())?;
    if &header[..8] != MAGIC
        || header[8..24] != *backup.as_bytes()
        || header[24..40] != *object.as_bytes()
        || header[40..72] != Sha256::digest(master)[..]
    {
        return Err(integrity());
    }
    let cipher = key(master, &header)?;
    let header_hash = Sha256::digest(header);
    let mut size = 0u64;
    let mut hash = Sha256::new();
    let mut sequence = 0u64;
    loop {
        let mut raw = [0u8; 4];
        input.read_exact(&mut raw).map_err(|_| integrity())?;
        let tag = u32::from_be_bytes(raw);
        let length = (tag & !FINAL) as usize;
        let final_record = tag & FINAL != 0;
        if (final_record && length != 40) || (!final_record && (length == 0 || length > CHUNK)) {
            return Err(integrity());
        }
        let mut bytes = Zeroizing::new(vec![0u8; length + 16]);
        input.read_exact(&mut bytes).map_err(|_| integrity())?;
        let plaintext = cipher
            .open_in_place(
                nonce(sequence)?,
                aead::Aad::from(aad(&header_hash, sequence, tag)),
                &mut bytes,
            )
            .map_err(|_| integrity())?;
        if final_record {
            let expected = u64::from_be_bytes(plaintext[..8].try_into().unwrap());
            let digest = hash.finalize();
            if expected != size || plaintext[8..] != digest[..] {
                return Err(integrity());
            }
            let mut trailing = [0u8; 1];
            if input.read(&mut trailing)? != 0 {
                return Err(integrity());
            }
            output.flush()?;
            return Ok(Summary {
                bytes: size,
                digest: format!("sha256:{digest:x}"),
            });
        }
        size = size
            .checked_add(length as u64)
            .filter(|n| *n <= maximum)
            .ok_or_else(Error::exhausted)?;
        hash.update(&*plaintext);
        output.write_all(plaintext)?;
        sequence += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sealed(data: &[u8], key: &[u8; 32], backup: Uuid, object: Uuid) -> Vec<u8> {
        let mut out = Vec::new();
        seal(data, &mut out, key, backup, object, data.len() as u64).unwrap();
        out
    }
    #[test]
    fn encrypted_objects_bind_identity_and_stream_boundaries() {
        let key = [41u8; 32];
        let backup = Uuid::new_v4();
        let object = Uuid::new_v4();
        for data in [
            vec![],
            vec![5; 1],
            vec![13; CHUNK],
            vec![73; CHUNK * 2 + 17],
        ] {
            let encoded = sealed(&data, &key, backup, object);
            let mut decoded = Vec::new();
            let result = open(
                encoded.as_slice(),
                &mut decoded,
                &key,
                backup,
                object,
                data.len() as u64,
            )
            .unwrap();
            assert_eq!(decoded, data);
            assert_eq!(result.bytes, data.len() as u64);
            assert_eq!(result.digest, crate::contracts::digest(&data));
            assert_ne!(encoded, sealed(&data, &key, backup, object));
            for end in [0, HEADER - 1, HEADER, encoded.len() - 17, encoded.len() - 1] {
                assert!(open(
                    &encoded[..end],
                    Vec::new(),
                    &key,
                    backup,
                    object,
                    data.len() as u64
                )
                .is_err());
            }
            let mut appended = encoded.clone();
            appended.push(0);
            assert!(open(
                appended.as_slice(),
                Vec::new(),
                &key,
                backup,
                object,
                data.len() as u64
            )
            .is_err());
            assert!(open(
                encoded.as_slice(),
                Vec::new(),
                &[42; 32],
                backup,
                object,
                data.len() as u64
            )
            .is_err());
            assert!(open(
                encoded.as_slice(),
                Vec::new(),
                &key,
                Uuid::new_v4(),
                object,
                data.len() as u64
            )
            .is_err());
            assert!(open(
                encoded.as_slice(),
                Vec::new(),
                &key,
                backup,
                Uuid::new_v4(),
                data.len() as u64
            )
            .is_err());
        }
    }
    #[test]
    fn tampering_reordering_and_length_limits_never_report_success() {
        let key = [45; 32];
        let backup = Uuid::new_v4();
        let object = Uuid::new_v4();
        let data = vec![67; CHUNK * 2 + 17];
        let encoded = sealed(&data, &key, backup, object);
        for offset in [
            0,
            8,
            24,
            40,
            72,
            HEADER,
            HEADER + 4,
            HEADER + 4 + CHUNK,
            encoded.len() - 1,
        ] {
            let mut bad = encoded.clone();
            bad[offset] ^= 1;
            assert!(open(
                bad.as_slice(),
                Vec::new(),
                &key,
                backup,
                object,
                data.len() as u64
            )
            .is_err());
        }
        let mut swapped = encoded.clone();
        let width = 4 + CHUNK + 16;
        let a = encoded[HEADER..HEADER + width].to_vec();
        swapped.copy_within(HEADER + width..HEADER + 2 * width, HEADER);
        swapped[HEADER + width..HEADER + 2 * width].copy_from_slice(&a);
        assert!(open(
            swapped.as_slice(),
            Vec::new(),
            &key,
            backup,
            object,
            data.len() as u64
        )
        .is_err());
        assert!(open(
            encoded.as_slice(),
            Vec::new(),
            &key,
            backup,
            object,
            data.len() as u64 - 1
        )
        .is_err());
        assert!(seal(
            data.as_slice(),
            Vec::new(),
            &key,
            backup,
            object,
            data.len() as u64 - 1
        )
        .is_err());
    }
}
