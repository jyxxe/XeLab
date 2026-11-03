//! Actual OCI content archives for the offline coordinator. This adapter does
//! not fence workloads, store encryption keys or declare a platform restore ready.
use super::{binary, strs, Runtime};
use crate::{
    artifact,
    config::require_host_admin,
    contracts::strict_json,
    error::{Error, Result},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::MetadataExt,
    process::Stdio,
    time::Duration,
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    process::{Child, Command},
};

const MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_IMAGES: usize = 128;
const BUFFER: usize = 65536;
const DEADLINE: Duration = Duration::from_secs(300);

#[path = "runtime_image_tar.rs"]
mod oci_tar;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageArchive {
    pub images: Vec<String>,
    pub bytes: u64,
    pub sha256: String,
}
fn references(images: &[String]) -> Result<()> {
    if images.is_empty() || images.len() > MAX_IMAGES {
        return Err(Error::invalid(
            "Image archive requires one to 128 immutable references",
        ));
    }
    for image in images {
        artifact::image_ref(image)?;
    }
    if images.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(Error::invalid(
            "Image archive references must be sorted and unique",
        ));
    }
    Ok(())
}
fn file_is_private(meta: &std::fs::Metadata) -> Result<()> {
    if !meta.is_file() || meta.uid() != 0 || meta.nlink() != 1 || meta.mode() & 0o077 != 0 {
        return Err(Error::invalid(
            "Image staging requires a root-owned private regular file",
        ));
    }
    Ok(())
}
fn same_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}
async fn copy_bounded(
    input: &mut (impl AsyncRead + Unpin),
    out: &mut (impl AsyncWrite + Unpin),
    maximum: u64,
) -> Result<(u64, String)> {
    let mut bytes = 0u64;
    let mut hash = Sha256::new();
    // Async futures may be nested by the CLI/coordinator. Keep bounded I/O
    // buffers off their stack-resident state, including in debug builds.
    let mut buffer = vec![0u8; BUFFER];
    loop {
        let n = input.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        bytes = bytes.checked_add(n as u64).ok_or_else(Error::exhausted)?;
        if bytes > maximum {
            return Err(Error::exhausted());
        }
        out.write_all(&buffer[..n]).await?;
        hash.update(&buffer[..n]);
    }
    out.flush().await?;
    Ok((bytes, format!("sha256:{:x}", hash.finalize())))
}
impl Runtime {
    fn image_command(&self, args: &[String], input: Stdio) -> Result<Child> {
        let mut command = Command::new(binary("ctr")?);
        command.args([
            "--address",
            self.config.text("runtime", "socket"),
            "--namespace",
            self.config.text("runtime", "namespace"),
        ]);
        Ok(command
            .args(args)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .stdin(input)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?)
    }

    /// Export real, already verified immutable image content to an empty private
    /// file. The offline coordinator owns its disk reservation and encryption.
    pub async fn export_images(
        &self,
        images: &[String],
        file: &mut File,
        maximum: u64,
    ) -> Result<ImageArchive> {
        require_host_admin()?;
        references(images)?;
        if maximum == 0 || maximum > MAX_BYTES {
            return Err(Error::invalid("Image archive exceeds its supported limit"));
        }
        file_is_private(&file.metadata().await?)?;
        if file.metadata().await?.len() != 0 {
            return Err(Error::conflict("Image archive output must be empty"));
        }
        file.rewind().await?;
        // containerd 2.x defaults to the transfer API, whose progress also uses
        // stdout. A binary archive needs the direct, synchronous local path.
        let mut args = strs(&["images", "export", "--local", "--skip-manifest-json", "-"]);
        args.extend_from_slice(images);
        let mut process: Option<Child> = None;
        let work = async {
            for image in images {
                self.verify_image(image, artifact::image_ref(image)?)
                    .await?;
            }
            process = Some(self.image_command(&args, Stdio::null())?);
            let child = process.as_mut().unwrap();
            let mut stdout = child
                .stdout
                .take()
                .ok_or_else(|| Error::unavailable("Image export output is unavailable"))?;
            let (bytes, sha256) =
                copy_bounded(&mut stdout, file, maximum)
                    .await
                    .map_err(|mut error| {
                        if error.code == "RESOURCE_EXHAUSTED" {
                            error.message = "Image export exceeded its admitted byte limit";
                        }
                        error
                    })?;
            if bytes == 0 || !child.wait().await?.success() {
                return Err(Error::unavailable("Image export did not complete"));
            }
            file.sync_all().await?;
            file_is_private(&file.metadata().await?)?;
            if file.metadata().await?.len() != bytes {
                return Err(Error::conflict("Image archive file changed"));
            }
            file.rewind().await?;
            oci_tar::verify(file, images, bytes).await?;
            for image in images {
                self.verify_image(image, artifact::image_ref(image)?)
                    .await?;
            }
            Ok(ImageArchive {
                images: images.to_vec(),
                bytes,
                sha256,
            })
        };
        let result = tokio::time::timeout(DEADLINE, work)
            .await
            .map_err(|_| Error::unavailable("Image export exceeded its deadline"));
        match result {
            Ok(Ok(value)) => Ok(value),
            other => {
                if let Some(child) = process.as_mut() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                other?
            }
        }
    }

    /// Verify the full staged archive before importing into an empty dedicated
    /// namespace. Rehash the imported bytes and verify every immutable manifest.
    /// Failure leaves that unpublished namespace for the coordinator to inspect;
    /// it never deletes shared containerd content or publishes a deployment.
    pub async fn import_images(&self, file: &mut File, expected: &ImageArchive) -> Result<()> {
        require_host_admin()?;
        references(&expected.images)?;
        if expected.bytes == 0 || expected.bytes > MAX_BYTES {
            return Err(Error::invalid("Image archive exceeds its supported limit"));
        }
        let before = file.metadata().await?;
        file_is_private(&before)?;
        if before.len() != expected.bytes {
            return Err(Error::invalid("Image archive length differs"));
        }
        let mut process: Option<Child> = None;
        let work = async {
            if !self.inventory().await?.is_empty()
                || !String::from_utf8(self.ctr(&strs(&["images", "list", "--quiet"])).await?)
                    .map_err(|_| Error::unavailable("Invalid image inventory"))?
                    .trim()
                    .is_empty()
            {
                return Err(Error::conflict(
                    "Image recovery requires an empty dedicated namespace",
                ));
            }
            file.rewind().await?;
            let first = copy_bounded(file, &mut tokio::io::sink(), expected.bytes).await?;
            if first != (expected.bytes, expected.sha256.clone())
                || !same_file(&before, &file.metadata().await?)
            {
                return Err(Error::invalid("Image archive digest differs before import"));
            }
            file.rewind().await?;
            oci_tar::verify(file, &expected.images, expected.bytes).await?;
            file.rewind().await?;
            process = Some(self.image_command(
                &strs(&["images", "import", "--local", "--no-unpack", "-"]),
                Stdio::piped(),
            )?);
            let child = process.as_mut().unwrap();
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| Error::unavailable("Image import input is unavailable"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| Error::unavailable("Image import output is unavailable"))?;
            let transfer = async {
                let feed = async {
                    let actual = copy_bounded(file, &mut stdin, expected.bytes).await?;
                    stdin.shutdown().await?;
                    drop(stdin);
                    Ok::<_, Error>(actual)
                };
                let receive = async {
                    let mut output = Vec::new();
                    stdout.take(2097153).read_to_end(&mut output).await?;
                    if output.len() > 2097152 {
                        return Err(Error::exhausted());
                    }
                    Ok::<_, Error>(())
                };
                let (actual, ()) = tokio::try_join!(feed, receive)?;
                if !child.wait().await?.success() {
                    return Err(Error::unavailable("Image import did not complete"));
                }
                if actual != (expected.bytes, expected.sha256.clone())
                    || !same_file(&before, &file.metadata().await?)
                {
                    return Err(Error::invalid("Image archive changed during import"));
                }
                Ok::<_, Error>(())
            }
            .await;
            if let Err(error) = transfer {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(error);
            }
            for image in &expected.images {
                self.verify_image(image, artifact::image_ref(image)?)
                    .await?;
            }
            Ok(())
        };
        let result = tokio::time::timeout(DEADLINE, work)
            .await
            .map_err(|_| Error::unavailable("Image import exceeded its deadline"));
        match result {
            Ok(Ok(())) => Ok(()),
            other => {
                if let Some(child) = process.as_mut() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                other?
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stream_limits_digest_and_immutable_reference_admission() {
        let good = format!("example.test/x@sha256:{}", "a".repeat(64));
        assert!(references(std::slice::from_ref(&good)).is_ok());
        assert!(references(&[]).is_err());
        assert!(references(&[good.clone(), good]).is_err());
        assert!(references(&["example.test/x:latest".into()]).is_err());
        let input = vec![43u8; BUFFER * 2 + 7];
        let mut output = Vec::new();
        let summary = copy_bounded(&mut input.as_slice(), &mut output, input.len() as u64)
            .await
            .unwrap();
        assert_eq!(output, input);
        assert_eq!(summary.0, input.len() as u64);
        assert_eq!(summary.1, format!("sha256:{:x}", Sha256::digest(&input)));
        let mut rejected = Vec::new();
        assert!(
            copy_bounded(&mut input.as_slice(), &mut rejected, (BUFFER + 3) as u64)
                .await
                .is_err()
        );
        assert!(rejected.len() <= BUFFER + 3);
    }
    #[tokio::test]
    async fn public_archive_futures_fit_the_standard_runtime_stack() {
        let t: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        let runtime = Runtime::new(
            crate::config::Config::from_value(serde_json::to_value(t).unwrap()).unwrap(),
        );
        let named = tempfile::NamedTempFile::new().unwrap();
        let mut file = File::from_std(named.reopen().unwrap());
        let images = vec![format!("example.test/image@sha256:{}", "a".repeat(64))];
        let exported = runtime.export_images(&images, &mut file, 1024);
        assert!(
            std::mem::size_of_val(&exported) < 32768,
            "export future must not embed streaming buffers"
        );
        drop(exported);
        let expected = ImageArchive {
            images,
            bytes: 512,
            sha256: format!("sha256:{}", "a".repeat(64)),
        };
        let imported = runtime.import_images(&mut file, &expected);
        assert!(
            std::mem::size_of_val(&imported) < 32768,
            "import future must not embed streaming buffers"
        );
    }
}
