//! Real one-time Minisign keys; payloads are text fixtures, never installers.
use std::{fs, path::PathBuf, process::Command};

use base64::{Engine, engine::general_purpose::STANDARD};
use giveaway_scout_release_tools::{
    Artifact, ReleaseInput, ReleaseProfile, TARGETS, prepare, verify_signature,
};
use minisign::KeyPair;
use sha2::{Digest, Sha256};

struct Fixture {
    directory: tempfile::TempDir,
    keys: KeyPair,
    input: ReleaseInput,
    input_path: PathBuf,
    public_path: PathBuf,
    output: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let keys = KeyPair::generate_unencrypted_keypair().unwrap();
        let public_path = directory.path().join("updater.pub");
        fs::write(
            &public_path,
            STANDARD.encode(keys.pk.to_box().unwrap().into_string()) + "\n",
        )
        .unwrap();
        let mut artifacts = Vec::new();
        for target in TARGETS {
            let name = match target {
                "darwin-aarch64" => "Giveaway Scout.app.tar.gz",
                "linux-aarch64" => "Giveaway Scout_1.2.3_aarch64.AppImage",
                "linux-x86_64" => "Giveaway Scout_1.2.3_amd64.AppImage",
                "windows-aarch64" => "Giveaway Scout_1.2.3_arm64-setup.exe",
                _ => "Giveaway Scout_1.2.3_x64-setup.exe",
            };
            fs::write(
                directory.path().join(name),
                format!("ephemeral {target} fixture - never an installer"),
            )
            .unwrap();
            let mut url = url::Url::parse("https://example.invalid/releases/1.2.3/").unwrap();
            url.path_segments_mut().unwrap().pop_if_empty().push(name);
            artifacts.push(Artifact {
                target: target.to_owned(),
                path: name.into(),
                signature_path: format!("{name}.sig").into(),
                url: url.to_string(),
            });
        }
        let input_path = directory.path().join("release.json");
        let output = directory.path().join("output");
        let fixture = Self {
            directory,
            keys,
            input_path,
            public_path,
            output,
            input: ReleaseInput {
                release_profile: ReleaseProfile::AllPlatforms,
                version: "1.2.3".to_owned(),
                previous_version: "1.2.2".to_owned(),
                notes: "Plain text: <img src=x onerror=alert(1)>".to_owned(),
                pub_date: "2020-01-01T00:00:00Z".to_owned(),
                artifacts,
            },
        };
        for index in 0..fixture.input.artifacts.len() {
            let name = fixture.input.artifacts[index].path.to_str().unwrap();
            fixture.resign(index, &format!("timestamp:0\tfile:{name}\tversion:1.2.3"));
        }
        fixture
    }

    fn resign(&self, index: usize, comment: &str) {
        let artifact = &self.input.artifacts[index];
        let bytes = fs::read(self.directory.path().join(&artifact.path)).unwrap();
        let signature = minisign::sign(
            Some(&self.keys.pk),
            &self.keys.sk,
            std::io::Cursor::new(bytes),
            Some(comment),
            Some("ephemeral test signature"),
        )
        .unwrap();
        fs::write(
            self.directory.path().join(&artifact.signature_path),
            STANDARD.encode(signature.into_string()),
        )
        .unwrap();
    }

    fn save(&self) {
        fs::write(
            &self.input_path,
            serde_json::to_vec_pretty(&self.input).unwrap(),
        )
        .unwrap();
    }

    fn prepare(&self) -> Result<giveaway_scout_release_tools::Report, String> {
        self.save();
        prepare(&self.input_path, &self.public_path, &self.output)
    }

    fn fails(&self, expected: &str) {
        let error = self.prepare().unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert!(
            !self.output.exists(),
            "failed verification must not create output"
        );
    }
}

#[test]
fn verifies_all_five_signatures_and_copies_the_exact_bytes_without_claiming_release_readiness() {
    let fixture = Fixture::new();
    let report = fixture.prepare().unwrap();
    assert!(report.updater_signatures_verified);
    assert_eq!(report.release_profile, ReleaseProfile::AllPlatforms);
    let key_text = fs::read_to_string(&fixture.public_path).unwrap();
    assert_eq!(
        report.updater_public_key_sha256,
        format!("{:x}", Sha256::digest(key_text.trim().as_bytes()))
    );
    let saved_report: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.output.join("verification.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved_report["updaterPublicKeySha256"],
        report.updater_public_key_sha256
    );
    assert!(
        !report.public_release_ready
            && !report.native_acceptance_verified
            && !report.platform_signing_verified
    );
    assert_eq!(report.files.len(), 5);
    let latest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.output.join("latest.json")).unwrap()).unwrap();
    assert_eq!(latest["version"], "1.2.3");
    assert_eq!(latest["notes"], fixture.input.notes);
    assert_eq!(latest["platforms"].as_object().unwrap().len(), 5);
    for file in report.files {
        let bytes = fs::read(fixture.output.join("packages").join(&file.filename)).unwrap();
        assert_eq!(
            bytes,
            fs::read(fixture.directory.path().join(&file.filename)).unwrap()
        );
        assert_eq!(file.sha256, format!("{:x}", Sha256::digest(&bytes)));
        assert!(!file.package_format_verified && !file.payload_architecture_verified);
        verify_signature(
            &fs::read_to_string(&fixture.public_path).unwrap(),
            latest["platforms"][&file.target]["signature"]
                .as_str()
                .unwrap(),
            &bytes,
            "1.2.3",
            &file.filename,
        )
        .unwrap();
    }
    assert!(!fixture.output.join("INCOMPLETE").exists());
}

#[test]
fn rejects_changed_package_bytes() {
    let fixture = Fixture::new();
    fs::write(
        fixture
            .directory
            .path()
            .join(&fixture.input.artifacts[0].path),
        b"changed",
    )
    .unwrap();
    fixture.fails("signature is invalid");
}

#[test]
fn rejects_another_public_key() {
    let fixture = Fixture::new();
    let other = KeyPair::generate_unencrypted_keypair().unwrap();
    fs::write(
        &fixture.public_path,
        STANDARD.encode(other.pk.to_box().unwrap().into_string()),
    )
    .unwrap();
    fixture.fails("signature is invalid");
}

#[test]
fn rejects_missing_and_oversized_signature_files() {
    for bytes in [Vec::new(), vec![b'x'; 8193]] {
        let fixture = Fixture::new();
        fs::write(
            fixture
                .directory
                .path()
                .join(&fixture.input.artifacts[0].signature_path),
            bytes,
        )
        .unwrap();
        assert!(fixture.prepare().is_err());
        assert!(!fixture.output.exists());
    }
}

#[test]
fn rejects_unversioned_mismatched_duplicate_and_wrong_filename_comments() {
    for comment in [
        "file:Giveaway Scout.app.tar.gz",
        "file:Giveaway Scout.app.tar.gz\tversion:0.1.0",
        "file:Giveaway Scout.app.tar.gz\tversion:1.2.3\tversion:1.2.3",
        "file:renamed.app.tar.gz\tversion:1.2.3",
    ] {
        let fixture = Fixture::new();
        fixture.resign(0, comment);
        assert!(fixture.prepare().is_err(), "{comment}");
        assert!(!fixture.output.exists());
    }
}

#[test]
fn verifies_the_global_signature_before_reading_trusted_metadata() {
    let fixture = Fixture::new();
    let path = fixture
        .directory
        .path()
        .join(&fixture.input.artifacts[0].signature_path);
    let decoded =
        String::from_utf8(STANDARD.decode(fs::read_to_string(&path).unwrap()).unwrap()).unwrap();
    let modified = decoded.replace("version:1.2.3", "version:9.9.9");
    assert_ne!(decoded, modified);
    fs::write(path, STANDARD.encode(modified)).unwrap();
    fixture.fails("signature is invalid");
}

#[test]
fn requires_exactly_one_package_per_planned_target() {
    for mode in 0..3 {
        let mut fixture = Fixture::new();
        match mode {
            0 => {
                fixture.input.artifacts.pop();
            }
            1 => fixture.input.artifacts[1].target = fixture.input.artifacts[0].target.clone(),
            _ => fixture.input.artifacts[0].target = "android-aarch64".to_owned(),
        }
        fixture.fails("release profile");
    }
}

#[test]
fn windows_profile_prepares_only_windows_x64_without_requiring_experimental_packages() {
    let mut fixture = Fixture::new();
    fixture.input.release_profile = ReleaseProfile::WindowsX64;
    fixture
        .input
        .artifacts
        .retain(|a| a.target == "windows-x86_64");
    let report = fixture.prepare().unwrap();
    assert_eq!(report.release_profile, ReleaseProfile::WindowsX64);
    assert_eq!(report.files.len(), 1);
    assert_eq!(report.files[0].target, "windows-x86_64");
    assert!(!report.public_release_ready && !report.native_acceptance_verified);
    let latest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.output.join("latest.json")).unwrap()).unwrap();
    assert_eq!(latest["platforms"].as_object().unwrap().len(), 1);
    assert!(latest["platforms"]["windows-x86_64"].is_object());
    assert_eq!(
        fs::read_dir(fixture.output.join("packages"))
            .unwrap()
            .count(),
        2
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.output.join("verification.json")).unwrap())
            .unwrap();
    assert_eq!(saved["releaseProfile"], "windows-x64");
}

#[test]
fn windows_profile_rejects_missing_extra_and_other_platform_packages() {
    for mode in 0..4 {
        let mut fixture = Fixture::new();
        fixture.input.release_profile = ReleaseProfile::WindowsX64;
        match mode {
            0 => fixture.input.artifacts.clear(),
            1 => {} // Five valid packages still exceed the selected scope.
            2 => fixture
                .input
                .artifacts
                .retain(|a| a.target == "darwin-aarch64"),
            _ => fixture
                .input
                .artifacts
                .retain(|a| a.target == "windows-aarch64"),
        }
        fixture.fails("release profile");
    }
}

#[test]
fn release_profile_is_required_and_unknown_profiles_are_not_inferred_from_files() {
    for profile in [None, Some("automatic")] {
        let fixture = Fixture::new();
        let mut input = serde_json::to_value(&fixture.input).unwrap();
        input.as_object_mut().unwrap().remove("releaseProfile");
        if let Some(profile) = profile {
            input["releaseProfile"] = profile.into();
        }
        fs::write(&fixture.input_path, serde_json::to_vec(&input).unwrap()).unwrap();
        assert!(
            prepare(&fixture.input_path, &fixture.public_path, &fixture.output)
                .unwrap_err()
                .contains("Invalid release input")
        );
        assert!(!fixture.output.exists());
    }
}

#[test]
fn requires_a_newer_version_not_an_equal_downgrade_or_metadata_only_bump() {
    for (version, previous) in [
        ("1.2.3", "1.2.3"),
        ("1.2.3", "2.0.0"),
        ("1.2.3+new", "1.2.3"),
        ("v1.2.3", "1.2.2"),
    ] {
        let mut fixture = Fixture::new();
        fixture.input.version = version.to_owned();
        fixture.input.previous_version = previous.to_owned();
        assert!(fixture.prepare().is_err());
        assert!(!fixture.output.exists());
    }
}

#[test]
fn rejects_unsafe_urls_and_url_filename_mismatch_without_network_requests() {
    for url in [
        "http://example.invalid/a",
        "file:///tmp/a",
        "https://user:secret@example.invalid/a",
        "https://example.invalid/a?token=secret",
        "https://example.invalid/a#fragment",
        "https://example.invalid/wrong.app.tar.gz",
    ] {
        let mut fixture = Fixture::new();
        fixture.input.artifacts[0].url = url.to_owned();
        assert!(fixture.prepare().is_err());
        assert!(!fixture.output.exists());
    }
}

#[test]
fn github_normalizes_only_hosted_names_while_signed_names_and_bytes_stay_verified() {
    let mut fixture = Fixture::new();
    fixture.input.release_profile = ReleaseProfile::WindowsX64;
    fixture
        .input
        .artifacts
        .retain(|a| a.target == "windows-x86_64");
    let original = fixture.input.artifacts[0].path.to_str().unwrap().to_owned();
    let hosted = original.replace(' ', ".");
    let url = format!(
        "https://github.com/gogolevmatvey/giveaway-scout/releases/download/v1.2.3/{hosted}"
    );
    fixture.input.artifacts[0].url = url.clone();
    let report = fixture.prepare().unwrap();
    assert_eq!(report.files[0].filename, original);
    assert_eq!(report.files[0].download_filename, hosted);
    let bytes = fs::read(fixture.output.join("packages").join(&hosted)).unwrap();
    assert_eq!(
        bytes,
        fs::read(fixture.directory.path().join(&original)).unwrap()
    );
    let signature = fs::read_to_string(
        fixture
            .output
            .join("packages")
            .join(format!("{hosted}.sig")),
    )
    .unwrap();
    let key = fs::read_to_string(&fixture.public_path).unwrap();
    verify_signature(&key, &signature, &bytes, "1.2.3", &original).unwrap();
    assert!(verify_signature(&key, &signature, &bytes, "1.2.3", &hosted).is_err());
    let latest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.output.join("latest.json")).unwrap()).unwrap();
    assert_eq!(latest["platforms"]["windows-x86_64"]["url"], url);
}

#[test]
fn github_filename_alias_cannot_change_the_host_owner_repository_tag_or_filename() {
    for url in [
        "https://example.invalid/gogolevmatvey/giveaway-scout/releases/download/v1.2.3/Giveaway.Scout_1.2.3_x64-setup.exe",
        "https://github.com/other/giveaway-scout/releases/download/v1.2.3/Giveaway.Scout_1.2.3_x64-setup.exe",
        "https://github.com/gogolevmatvey/other/releases/download/v1.2.3/Giveaway.Scout_1.2.3_x64-setup.exe",
        "https://github.com/gogolevmatvey/giveaway-scout/releases/download/v1.2.4/Giveaway.Scout_1.2.3_x64-setup.exe",
        "https://github.com:8443/gogolevmatvey/giveaway-scout/releases/download/v1.2.3/Giveaway.Scout_1.2.3_x64-setup.exe",
        "https://github.com/gogolevmatvey/giveaway-scout/releases/download/v1.2.3/Other.Scout_1.2.3_x64-setup.exe",
    ] {
        let mut fixture = Fixture::new();
        fixture.input.release_profile = ReleaseProfile::WindowsX64;
        fixture
            .input
            .artifacts
            .retain(|a| a.target == "windows-x86_64");
        fixture.input.artifacts[0].url = url.to_owned();
        fixture.fails("URL basename");
    }
}

#[test]
fn rejects_traversal_absolute_paths_and_nonportable_separators() {
    for path in [
        PathBuf::from("../outside.app.tar.gz"),
        PathBuf::from("dir\\file.app.tar.gz"),
        std::env::temp_dir().join("outside.app.tar.gz"),
    ] {
        let mut fixture = Fixture::new();
        fixture.input.artifacts[0].path = path;
        fixture.fails("portable and relative");
    }
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_even_when_they_point_inside_the_input_directory() {
    let mut fixture = Fixture::new();
    let original = fixture
        .directory
        .path()
        .join(&fixture.input.artifacts[0].path);
    std::os::unix::fs::symlink(original, fixture.directory.path().join("link.app.tar.gz")).unwrap();
    fixture.input.artifacts[0].path = "link.app.tar.gz".into();
    fixture.fails("symlinks");
}

#[test]
fn rejects_wrong_artifact_kinds() {
    let mut fixture = Fixture::new();
    fixture.input.artifacts[0].path = fixture.input.artifacts[1].path.clone();
    fixture.fails("artifact kind");
}

#[test]
fn never_overwrites_existing_output() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.output).unwrap();
    fs::write(fixture.output.join("keep.txt"), b"existing user data").unwrap();
    assert!(fixture.prepare().unwrap_err().contains("already exists"));
    assert_eq!(
        fs::read(fixture.output.join("keep.txt")).unwrap(),
        b"existing user data"
    );
}

#[test]
fn output_is_repeatable_with_the_same_inputs_even_in_a_different_input_order() {
    let mut fixture = Fixture::new();
    fixture.prepare().unwrap();
    let first = fs::read(fixture.output.join("latest.json")).unwrap();
    let first_report = fs::read(fixture.output.join("verification.json")).unwrap();
    fixture.output = fixture.directory.path().join("second-output");
    fixture.input.artifacts.reverse();
    fixture.prepare().unwrap();
    assert_eq!(first, fs::read(fixture.output.join("latest.json")).unwrap());
    assert_eq!(
        first_report,
        fs::read(fixture.output.join("verification.json")).unwrap()
    );
}

#[test]
fn enforces_note_and_date_limits() {
    let mut fixture = Fixture::new();
    fixture.input.notes = "x".repeat(20001);
    fixture.fails("notes");
    fixture.input.notes.clear();
    fixture.input.pub_date = "2099-01-01T00:00:00Z".to_owned();
    fixture.fails("future");
    fixture.input.pub_date = "not a date".to_owned();
    fixture.fails("pubDate");
}

#[test]
fn actual_cli_prepares_only_a_local_snapshot_and_rejects_unknown_arguments() {
    let mut fixture = Fixture::new();
    fixture.input.release_profile = ReleaseProfile::WindowsX64;
    fixture
        .input
        .artifacts
        .retain(|a| a.target == "windows-x86_64");
    fixture.save();
    let output = Command::new(env!("CARGO_BIN_EXE_giveaway-scout-release"))
        .arg("prepare")
        .arg("--input")
        .arg(&fixture.input_path)
        .arg("--public-key")
        .arg(&fixture.public_path)
        .arg("--output")
        .arg(&fixture.output)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("NOT established"));
    assert!(fixture.output.join("latest.json").is_file());
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_giveaway-scout-release"))
            .args(["prepare", "--publish", "now"])
            .output()
            .unwrap()
            .status
            .success()
    );
}
