//! Authenticated update metadata and packages, independent of the GUI runtime.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use url::Url;

pub const MAX_PACKAGE_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_NOTES_CHARS: usize = 20_000;

#[derive(Clone, Copy, Debug)]
pub struct UpdateConfig {
    pub endpoint: &'static str,
    pub public_key: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/release_config.rs"));

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCandidate {
    pub version: String,
    pub notes: String,
    pub url: Url,
    pub signature: String,
    pub filename: String,
}

#[derive(Deserialize)]
struct Platform {
    url: Url,
    signature: String,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: String,
    platforms: BTreeMap<String, Platform>,
}

pub fn secure_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

pub fn client_policy(client: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    client
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if secure_url(attempt.url()) && attempt.previous().len() < 5 {
                attempt.follow()
            } else {
                attempt.error("unsafe or excessive update redirect")
            }
        }))
}

fn decode(text: &str) -> Result<String, String> {
    String::from_utf8(
        STANDARD
            .decode(text.trim())
            .map_err(|_| "Invalid base64".to_owned())?,
    )
    .map_err(|_| "Invalid UTF-8 in signature/key".to_owned())
}

fn signed_field<'a>(comment: &'a str, prefix: &str) -> Result<&'a str, String> {
    let mut values = comment
        .split('\t')
        .filter_map(|part| part.strip_prefix(prefix));
    let value = values
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Missing, empty or duplicate signed field: {prefix}"))?;
    if values.next().is_some() {
        return Err(format!(
            "Missing, empty or duplicate signed field: {prefix}"
        ));
    }
    Ok(value)
}

/// Verify the package AND global signature before trusting version/filename.
pub fn verify_signature(
    key: &str,
    signature: &str,
    bytes: &[u8],
    version: &str,
    filename: &str,
) -> Result<(), String> {
    let key = PublicKey::decode(&decode(key)?).map_err(|_| "Invalid public key".to_owned())?;
    let signature = Signature::decode(&decode(signature)?)
        .map_err(|_| "Invalid signature encoding".to_owned())?;
    key.verify(bytes, &signature, true)
        .map_err(|_| "Package or trusted comment signature is invalid".to_owned())?;
    let signed = Version::parse(
        signed_field(signature.trusted_comment(), "version:")?.trim_start_matches('v'),
    )
    .map_err(|_| "Invalid signed version".to_owned())?;
    let announced = Version::parse(version).map_err(|_| "Invalid announced version".to_owned())?;
    if signed != announced {
        return Err("Signed version does not match announced version".to_owned());
    }
    if signed_field(signature.trusted_comment(), "file:")? != filename {
        return Err("Signed filename does not match package filename".to_owned());
    }
    Ok(())
}

pub fn parse_manifest(
    bytes: &[u8],
    current_version: &str,
) -> Result<Option<UpdateCandidate>, String> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err("Update manifest exceeds the size limit".to_owned());
    }
    let manifest: Manifest =
        serde_json::from_slice(bytes).map_err(|_| "Invalid update manifest".to_owned())?;
    let current =
        Version::parse(current_version).map_err(|_| "Invalid current version".to_owned())?;
    let available =
        Version::parse(&manifest.version).map_err(|_| "Invalid update version".to_owned())?;
    if available <= current {
        return Ok(None);
    }
    let target = manifest
        .platforms
        .get("windows-x86_64")
        .ok_or_else(|| "Update does not support Windows x64".to_owned())?;
    if !secure_url(&target.url) {
        return Err("Update package requires a safe HTTPS URL".to_owned());
    }
    if target.signature.len() > 16_384 {
        return Err("Update signature exceeds the size limit".to_owned());
    }
    let filename = format!("Giveaway Scout_{}_x64-setup.exe", manifest.version);
    // GitHub replaces spaces in uploaded asset names with dots. The signature
    // remains bound to the original local filename, as in release-tools.
    let encoded_filename = target
        .url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or("");
    let decoded_filename = filename.replace(' ', "%20");
    if encoded_filename != decoded_filename && encoded_filename != filename.replace(' ', ".") {
        return Err("Unexpected Windows update package filename".to_owned());
    }
    Ok(Some(UpdateCandidate {
        version: manifest.version,
        notes: manifest.notes.chars().take(MAX_NOTES_CHARS).collect(),
        url: target.url.clone(),
        signature: target.signature.clone(),
        filename,
    }))
}

async fn bounded_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let declared_length = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
        || declared_length.is_some_and(|size| size > limit as u64)
    {
        return Err("Update response exceeds the size limit".to_owned());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("Update response exceeds the size limit".to_owned());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub async fn check(
    config: UpdateConfig,
    current_version: &str,
) -> Result<Option<UpdateCandidate>, String> {
    let endpoint = Url::parse(config.endpoint).map_err(|_| "Invalid update endpoint".to_owned())?;
    if !secure_url(&endpoint) {
        return Err("Update endpoint requires HTTPS".to_owned());
    }
    PublicKey::decode(&decode(config.public_key)?)
        .map_err(|_| "Invalid update public key".to_owned())?;
    let response = client_policy(reqwest::Client::builder())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?
        .get(endpoint)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    parse_manifest(
        &bounded_response(response, MAX_MANIFEST_BYTES).await?,
        current_version,
    )
}

pub async fn download(
    config: UpdateConfig,
    candidate: &UpdateCandidate,
) -> Result<Vec<u8>, String> {
    if !secure_url(&candidate.url) {
        return Err("Update package requires HTTPS".to_owned());
    }
    let response = client_policy(reqwest::Client::builder())
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| error.to_string())?
        .get(candidate.url.clone())
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let bytes = bounded_response(response, MAX_PACKAGE_BYTES).await?;
    verify_signature(
        config.public_key,
        &candidate.signature,
        &bytes,
        &candidate.version,
        &candidate.filename,
    )?;
    Ok(bytes)
}

/// A private, authenticated setup with a read handle that denies writes on
/// Windows until CreateProcess has opened the executable image.
pub struct StagedInstaller {
    _file: File,
    directory: tempfile::TempDir,
    path: PathBuf,
}

pub fn stage_installer(
    public_key: &str,
    signature: &str,
    version: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<StagedInstaller, String> {
    let expected = format!("Giveaway Scout_{version}_x64-setup.exe");
    if filename != expected
        || Version::parse(version).is_err()
        || Path::new(filename).file_name() != Some(std::ffi::OsStr::new(filename))
    {
        return Err("Invalid installer filename/version".to_owned());
    }
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err("Update package exceeds the size limit".to_owned());
    }
    verify_signature(public_key, signature, bytes, version, filename)?;
    let directory = tempfile::Builder::new()
        .prefix("giveaway-scout-update-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let path = directory.path().join(filename);
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| error.to_string())?;
    writer
        .write_all(bytes)
        .and_then(|()| writer.sync_all())
        .map_err(|error| error.to_string())?;
    drop(writer);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1); // FILE_SHARE_READ: another process cannot modify it.
    }
    let mut file = options.open(&path).map_err(|error| error.to_string())?;
    let mut staged = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_PACKAGE_BYTES as u64 + 1)
        .read_to_end(&mut staged)
        .map_err(|error| error.to_string())?;
    verify_signature(public_key, signature, &staged, version, filename)?;
    Ok(StagedInstaller {
        directory,
        path,
        _file: file,
    })
}

pub fn installer_arguments(parent_pid: u32, restart_gui: bool) -> Vec<String> {
    let mut arguments = vec![
        "/P".into(),
        "/UPDATE".into(),
        format!("/WAITPID={parent_pid}"),
    ];
    if restart_gui {
        arguments.push("/R".into());
    }
    arguments
}

impl StagedInstaller {
    /// Launch first, then let the caller shut down. Spawn failure therefore
    /// leaves the existing monitoring runtime usable. Setup waits for this PID.
    pub fn launch(self, parent_pid: u32, restart_gui: bool) -> Result<(), String> {
        #[cfg(windows)]
        {
            std::process::Command::new(&self.path)
                .args(installer_arguments(parent_pid, restart_gui))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|error| error.to_string())?;
            let _directory = self.directory.keep();
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = (&self.path, &self.directory, parent_pid, restart_gui);
            Err("Installing updates this way is supported on Windows only".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn signed(comment: &str) -> (String, String, Vec<u8>) {
        let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let bytes = b"ephemeral signed fixture, never an executable".to_vec();
        let signature = minisign::sign(
            Some(&keys.pk),
            &keys.sk,
            Cursor::new(&bytes),
            Some(comment),
            None,
        )
        .unwrap();
        (
            STANDARD.encode(keys.pk.to_box().unwrap().into_string()),
            STANDARD.encode(signature.to_string()),
            bytes,
        )
    }

    #[test]
    fn authenticates_package_and_signed_metadata() {
        let (key, sig, bytes) = signed("timestamp:0\tfile:setup.exe\tversion:1.2.3");
        verify_signature(&key, &sig, &bytes, "1.2.3", "setup.exe").unwrap();
        assert!(verify_signature(&key, &sig, b"changed", "1.2.3", "setup.exe").is_err());
        assert!(verify_signature(&key, &sig, &bytes, "1.2.4", "setup.exe").is_err());
        assert!(verify_signature(&key, &sig, &bytes, "1.2.3", "different.exe").is_err());
        let (wrong, _, _) = signed("file:setup.exe\tversion:1.2.3");
        assert!(verify_signature(&wrong, &sig, &bytes, "1.2.3", "setup.exe").is_err());
        for comment in [
            "file:setup.exe",
            "file:setup.exe\tversion:1.2.3\tversion:1.2.3",
            "file:setup.exe\tfile:setup.exe\tversion:1.2.3",
        ] {
            let (key, sig, bytes) = signed(comment);
            assert!(verify_signature(&key, &sig, &bytes, "1.2.3", "setup.exe").is_err());
        }
    }

    fn manifest(version: &str, url: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"version":version,"platforms":{"windows-x86_64":{"url":url,"signature":"fixture"}}})).unwrap()
    }

    #[test]
    fn checks_version_target_and_filename() {
        let url = "https://example.invalid/Giveaway.Scout_1.2.3_x64-setup.exe";
        assert!(
            parse_manifest(&manifest("1.2.3", url), "1.2.2")
                .unwrap()
                .is_some()
        );
        assert!(
            parse_manifest(&manifest("1.2.3", url), "1.2.3")
                .unwrap()
                .is_none()
        );
        assert!(
            parse_manifest(&manifest("1.2.3", url), "1.3.0")
                .unwrap()
                .is_none()
        );
        assert!(
            parse_manifest(
                &manifest("1.2.3", "http://example.invalid/setup.exe"),
                "1.2.2"
            )
            .is_err()
        );
        assert!(
            parse_manifest(
                &manifest("1.2.3", "https://example.invalid/other.exe"),
                "1.2.2"
            )
            .is_err()
        );
        assert!(parse_manifest(&vec![b' '; MAX_MANIFEST_BYTES + 1], "1.2.2").is_err());
        assert!(parse_manifest(br#"{"version":"1.2.3","platforms":{}}"#, "1.2.2").is_err());
    }

    #[test]
    fn rejects_unsafe_transport() {
        for value in [
            "http://example.invalid/a",
            "https://user@example.invalid/a",
            "https://example.invalid/a#fragment",
        ] {
            assert!(!secure_url(&Url::parse(value).unwrap()));
        }
        assert!(secure_url(
            &Url::parse("https://example.invalid/a").unwrap()
        ));
    }

    #[tokio::test]
    async fn bounds_content_length_and_actual_body_independently() {
        let response = reqwest::Response::from(
            http::Response::builder()
                .header("content-length", "100")
                .body(b"small".to_vec())
                .unwrap(),
        );
        assert!(bounded_response(response, 16).await.is_err());
        let response = reqwest::Response::from(http::Response::new(vec![b'x'; 17]));
        assert!(bounded_response(response, 16).await.is_err());
        let response = reqwest::Response::from(http::Response::new(b"small".to_vec()));
        assert_eq!(bounded_response(response, 16).await.unwrap(), b"small");
        let error = client_policy(reqwest::Client::builder())
            .build()
            .unwrap()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err();
        assert!(error.is_builder());
    }

    #[test]
    fn installer_is_verified_and_handoff_is_explicit() {
        let filename = "Giveaway Scout_1.2.3_x64-setup.exe";
        let (key, sig, bytes) = signed(&format!("timestamp:0\tfile:{filename}\tversion:1.2.3"));
        let staged = stage_installer(&key, &sig, "1.2.3", filename, &bytes).unwrap();
        assert_eq!(std::fs::read(&staged.path).unwrap(), bytes);
        let directory = staged.directory.path().to_path_buf();
        drop(staged);
        assert!(!directory.exists());
        assert!(stage_installer(&key, &sig, "1.2.3", filename, b"changed").is_err());
        assert_eq!(
            installer_arguments(42, false),
            ["/P", "/UPDATE", "/WAITPID=42"]
        );
        assert_eq!(
            installer_arguments(42, true),
            ["/P", "/UPDATE", "/WAITPID=42", "/R"]
        );
    }
}
