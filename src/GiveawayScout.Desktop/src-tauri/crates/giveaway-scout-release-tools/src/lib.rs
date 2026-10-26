//! Offline updater verification/staging, not publication or native acceptance.
//! No private-key, HTTP, installer or application-profile API exists here.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const TARGETS: [&str; 5] = [
    "darwin-aarch64",
    "linux-aarch64",
    "linux-x86_64",
    "windows-aarch64",
    "windows-x86_64",
];
pub const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseProfile {
    WindowsX64,
    AllPlatforms,
}

impl ReleaseProfile {
    pub const fn targets(self) -> &'static [&'static str] {
        match self {
            Self::WindowsX64 => &["windows-x86_64"],
            Self::AllPlatforms => &TARGETS,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifact {
    pub target: String,
    pub path: PathBuf,
    pub signature_path: PathBuf,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseInput {
    // Explicit and required: missing packages must never silently choose a
    // smaller release scope than the owner intended.
    pub release_profile: ReleaseProfile,
    pub version: String,
    pub previous_version: String,
    pub notes: String,
    pub pub_date: String,
    pub artifacts: Vec<Artifact>,
}

#[derive(Serialize)]
struct Platform {
    url: String,
    signature: String,
}
#[derive(Serialize)]
struct Manifest {
    version: String,
    notes: String,
    pub_date: String,
    platforms: BTreeMap<String, Platform>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedArtifact {
    pub target: String,
    pub filename: String,
    /// Hosted filename; the signature is still checked against `filename`.
    pub download_filename: String,
    pub size: u64,
    pub sha256: String,
    pub package_format_verified: bool,
    pub payload_architecture_verified: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: u32,
    pub release_profile: ReleaseProfile,
    pub version: String,
    /// SHA-256 of the trimmed Tauri base64 public-key text, not a private key.
    pub updater_public_key_sha256: String,
    pub updater_signatures_verified: bool,
    pub platform_signing_verified: bool,
    pub native_acceptance_verified: bool,
    pub public_release_ready: bool,
    pub files: Vec<VerifiedArtifact>,
}

fn decode(text: &str) -> Result<String> {
    let bytes = STANDARD
        .decode(text.trim())
        .map_err(|_| "Invalid base64".to_owned())?;
    String::from_utf8(bytes).map_err(|_| "Invalid UTF-8 in signature/key".to_owned())
}

fn field<'a>(comment: &'a str, prefix: &str) -> Result<&'a str> {
    let values: Vec<_> = comment
        .split('\t')
        .filter_map(|part| part.strip_prefix(prefix))
        .collect();
    if values.len() != 1 || values[0].is_empty() {
        return Err(format!(
            "Missing, empty or duplicate signed field: {prefix}"
        ));
    }
    Ok(values[0])
}

/// Real Minisign verification, including its global signature, before reading
/// trusted comments. Compatible with the runtime verifier in Tauri Updater.
pub fn verify_signature(
    key: &str,
    signature: &str,
    bytes: &[u8],
    version: &str,
    filename: &str,
) -> Result<()> {
    let key = PublicKey::decode(&decode(key)?).map_err(|_| "Invalid public key".to_owned())?;
    let signature = Signature::decode(&decode(signature)?)
        .map_err(|_| "Invalid signature encoding".to_owned())?;
    key.verify(bytes, &signature, true)
        .map_err(|_| "Package or trusted comment signature is invalid".to_owned())?;
    let signed = field(signature.trusted_comment(), "version:")?;
    let signed = Version::parse(signed.trim_start_matches('v'))
        .map_err(|_| "Invalid signed version".to_owned())?;
    let announced = Version::parse(version).map_err(|_| "Invalid announced version".to_owned())?;
    if signed != announced {
        return Err("Signed version does not match announced version".to_owned());
    }
    if field(signature.trusted_comment(), "file:")? != filename {
        return Err("Signed filename does not match package filename".to_owned());
    }
    Ok(())
}

fn read_limited(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let file =
        File::open(path).map_err(|error| format!("Cannot open {}: {error}", path.display()))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err("File exceeds size limit or is not regular".to_owned());
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > maximum {
        return Err("File exceeds size limit".to_owned());
    }
    Ok(bytes)
}

fn text_file(path: &Path, maximum: u64) -> Result<String> {
    String::from_utf8(read_limited(path, maximum)?)
        .map_err(|_| "Expected a UTF-8 text file".to_owned())
}

fn source_file(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative.to_string_lossy().contains('\\')
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(
            "Package/signature path must be portable and relative without traversal".to_owned(),
        );
    }
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        if fs::symlink_metadata(&path)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Package/signature symlinks are not accepted".to_owned());
        }
    }
    let resolved = path.canonicalize().map_err(|error| error.to_string())?;
    if !resolved.starts_with(root) {
        return Err("Package/signature escapes input directory".to_owned());
    }
    Ok(resolved)
}

fn filename(path: &Path) -> Result<String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid package filename")?;
    if name.is_empty()
        || name.trim() != name
        || name.starts_with('.')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " ._-".contains(c))
    {
        return Err("Package filename is not portable".to_owned());
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|number| number.len() == 1 && number.as_bytes()[0].is_ascii_digit())
        })
    {
        return Err("Reserved Windows device filename is not portable".to_owned());
    }
    Ok(name.to_owned())
}

fn valid_kind(target: &str, name: &str) -> bool {
    match target {
        "darwin-aarch64" => name.ends_with(".app.tar.gz"),
        "linux-aarch64" | "linux-x86_64" => name.ends_with(".AppImage"),
        "windows-aarch64" => name.ends_with("_arm64-setup.exe"),
        "windows-x86_64" => name.ends_with("_x64-setup.exe"),
        _ => false,
    }
}

fn safe_url(text: &str, filename: &str, version: &str) -> Result<(String, String)> {
    let url = url::Url::parse(text).map_err(|_| "Invalid package URL".to_owned())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(
            "Package URL must be stable HTTPS without credentials/query/fragment".to_owned(),
        );
    }
    let mut expected = url.clone();
    expected
        .path_segments_mut()
        .map_err(|_| "Invalid package URL path".to_owned())?
        .pop()
        .push(filename);
    if expected == url {
        return Ok((url.to_string(), filename.to_owned()));
    }
    // GitHub normalizes spaces in uploaded asset names to dots. This changes
    // only the transport filename, never the bytes, signed filename or version.
    // Restrict the alias to our exact repository and the signed release tag.
    let hosted_name = filename.replace(' ', ".");
    let mut github =
        url::Url::parse("https://github.com/gogolevmatvey/giveaway-scout/releases/download/")
            .map_err(|_| "Invalid GitHub release URL".to_owned())?;
    github
        .path_segments_mut()
        .map_err(|_| "Invalid GitHub release path".to_owned())?
        .pop_if_empty()
        .push(&format!("v{version}"))
        .push(&hosted_name);
    if hosted_name != filename && github == url {
        return Ok((url.to_string(), hosted_name));
    }
    Err("URL basename does not match the signed package filename".to_owned())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())
}

/// Produces only a new local staging snapshot. Publication, platform signing,
/// payload architecture and native update acceptance remain separate gates.
pub fn prepare(input_path: &Path, public_key_path: &Path, output: &Path) -> Result<Report> {
    if fs::symlink_metadata(output).is_ok() {
        return Err("Output already exists; refusing to replace it".to_owned());
    }
    let input_path = input_path
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let root = input_path.parent().ok_or("Input directory is missing")?;
    let input: ReleaseInput = serde_json::from_slice(&read_limited(&input_path, 1024 * 1024)?)
        .map_err(|error| format!("Invalid release input: {error}"))?;
    let version =
        Version::parse(&input.version).map_err(|_| "Invalid release version".to_owned())?;
    let previous = Version::parse(&input.previous_version)
        .map_err(|_| "Invalid previous version".to_owned())?;
    if version.to_string() != input.version || !version.cmp_precedence(&previous).is_gt() {
        return Err(
            "Release must be canonical semver and strictly newer than previousVersion".to_owned(),
        );
    }
    if input.notes.chars().count() > 20_000 {
        return Err("Release notes exceed the size limit".to_owned());
    }
    let date = OffsetDateTime::parse(&input.pub_date, &Rfc3339)
        .map_err(|_| "Invalid pubDate".to_owned())?;
    if date.unix_timestamp() < 0 || date > OffsetDateTime::now_utc() + time::Duration::minutes(10) {
        return Err("Release date is invalid or in the future".to_owned());
    }
    let targets: BTreeSet<_> = input
        .artifacts
        .iter()
        .map(|artifact| artifact.target.as_str())
        .collect();
    let expected: BTreeSet<_> = input.release_profile.targets().iter().copied().collect();
    if input.artifacts.len() != expected.len() || targets != expected {
        return Err(
            "Exactly one updater artifact for every target in the selected release profile is required"
                .to_owned(),
        );
    }
    let key = text_file(public_key_path, 8192)?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stage = tempfile::Builder::new()
        .prefix(".giveaway-scout-updater-")
        .tempdir_in(parent)
        .map_err(|error| error.to_string())?;
    let packages = stage.path().join("packages");
    fs::create_dir(&packages).map_err(|error| error.to_string())?;
    let mut manifest = Manifest {
        version: input.version.clone(),
        notes: input.notes,
        pub_date: date.format(&Rfc3339).map_err(|error| error.to_string())?,
        platforms: BTreeMap::new(),
    };
    let mut report = Report {
        schema_version: 1,
        release_profile: input.release_profile,
        version: input.version.clone(),
        updater_public_key_sha256: format!("{:x}", Sha256::digest(key.trim().as_bytes())),
        updater_signatures_verified: true,
        platform_signing_verified: false,
        native_acceptance_verified: false,
        public_release_ready: false,
        files: Vec::new(),
    };
    let mut artifacts = input.artifacts;
    artifacts.sort_by(|left, right| left.target.cmp(&right.target));
    let mut names = BTreeSet::new();
    let mut download_names = BTreeSet::new();
    for artifact in artifacts {
        let path = source_file(root, &artifact.path)?;
        let name = filename(&path)?;
        if !valid_kind(&artifact.target, &name) || !names.insert(name.to_ascii_lowercase()) {
            return Err("Wrong updater artifact kind or duplicate filename".to_owned());
        }
        let (url, download_filename) = safe_url(&artifact.url, &name, &input.version)?;
        if !download_names.insert(download_filename.to_ascii_lowercase()) {
            return Err("Duplicate hosted package filename".to_owned());
        }
        let bytes = read_limited(&path, MAX_PACKAGE_BYTES)?;
        if bytes.is_empty() {
            return Err("Empty updater package".to_owned());
        }
        let signature = text_file(&source_file(root, &artifact.signature_path)?, 8192)?
            .trim()
            .to_owned();
        verify_signature(&key, &signature, &bytes, &input.version, &name)?;
        // Copy the exact bytes verified above, not a second read of the source.
        write_new(&packages.join(&download_filename), &bytes)?;
        write_new(
            &packages.join(format!("{download_filename}.sig")),
            signature.as_bytes(),
        )?;
        report.files.push(VerifiedArtifact {
            target: artifact.target.clone(),
            filename: name,
            download_filename,
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            package_format_verified: false,
            payload_architecture_verified: false,
        });
        manifest
            .platforms
            .insert(artifact.target, Platform { url, signature });
    }
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    let report_bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    // All verification is finished before any caller-visible output is created.
    // A commit I/O failure leaves an explicit marker; existing output is never
    // overwritten/deleted. Metadata is committed last with no-clobber semantics.
    fs::create_dir(output).map_err(|error| error.to_string())?;
    let marker = output.join("INCOMPLETE");
    write_new(
        &marker,
        b"Do not publish: local snapshot commit is incomplete.\n",
    )?;
    let destination = output.join("packages");
    fs::create_dir(&destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(&packages).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let mut source = File::open(entry.path()).map_err(|error| error.to_string())?;
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination.join(entry.file_name()))
            .map_err(|error| error.to_string())?;
        std::io::copy(&mut source, &mut target)
            .and_then(|_| target.sync_all())
            .map_err(|error| error.to_string())?;
    }
    write_new(&output.join("verification.json"), &report_bytes)?;
    let mut metadata =
        tempfile::NamedTempFile::new_in(output).map_err(|error| error.to_string())?;
    metadata
        .write_all(&manifest_bytes)
        .and_then(|()| metadata.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    metadata
        .persist_noclobber(output.join("latest.json"))
        .map_err(|error| error.to_string())?;
    fs::remove_file(marker).map_err(|error| error.to_string())?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_reserved_windows_devices_on_every_host() {
        for name in [
            "CON.AppImage",
            "nul.app.tar.gz",
            "PRN.AppImage",
            "COM1.AppImage",
            "LPT9.app.tar.gz",
        ] {
            assert!(filename(Path::new(name)).is_err(), "{name}");
        }
        assert!(filename(Path::new("Giveaway Scout_1.2.3_amd64.AppImage")).is_ok());
    }

    #[test]
    fn canonicalizes_url_spaces_without_allowing_a_different_filename() {
        assert_eq!(
            safe_url(
                "https://example.invalid/Giveaway Scout.AppImage",
                "Giveaway Scout.AppImage",
                "1.2.3"
            )
            .unwrap(),
            (
                "https://example.invalid/Giveaway%20Scout.AppImage".to_owned(),
                "Giveaway Scout.AppImage".to_owned()
            )
        );
        assert!(
            safe_url(
                "https://example.invalid/other.AppImage",
                "Giveaway Scout.AppImage",
                "1.2.3"
            )
            .is_err()
        );
    }
}
