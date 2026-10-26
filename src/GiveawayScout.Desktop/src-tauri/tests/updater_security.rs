//! Real signature/version verification. Mock runtime is only application plumbing.
//! No network, installer, window, profile database or production signing key.
use base64::{Engine, engine::general_purpose::STANDARD};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

const PAYLOAD: &[u8] = b"Giveaway Scout updater verification fixture - never an installer";

fn signed_fixture(comment: &str) -> (String, String) {
    let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let key = STANDARD.encode(keys.pk.to_box().unwrap().into_string());
    let signature = minisign::sign(
        Some(&keys.pk),
        &keys.sk,
        std::io::Cursor::new(PAYLOAD),
        Some(comment),
        Some("ephemeral test fixture"),
    )
    .unwrap();
    (key, STANDARD.encode(signature.into_string()))
}

fn fixture_updater(key: &str) -> Updater {
    fixture_updater_for_target(key, None)
}

fn fixture_updater_for_target(key: &str, target: Option<&str>) -> Updater {
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().plugins.0.insert("updater".to_owned(), serde_json::json!({
        "pubkey":key, "requireSignedVersion":true, "endpoints":["https://example.invalid/never-requested"]
    }));
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    let builder = app.updater_builder();
    let builder = if let Some(target) = target {
        builder.target(target)
    } else {
        builder
    };
    builder.build().unwrap()
}

fn fixture_update(key: &str, signature: &str) -> Update {
    fixture_updater(key).restore_update(serde_json::json!({
        "version":"1.2.3", "url":"https://example.invalid/never-downloaded", "signature":signature
    })).unwrap().unwrap()
}

#[test]
fn real_updater_verifies_signed_bytes_but_rejects_tampering_and_missing_signatures() {
    let (key, signature) = signed_fixture("timestamp:0\tfile:fixture\tversion:1.2.3");
    fixture_update(&key, &signature).verify(PAYLOAD).unwrap();
    giveaway_scout_release_tools::verify_signature(&key, &signature, PAYLOAD, "1.2.3", "fixture")
        .unwrap();
    assert!(
        giveaway_scout_release_tools::verify_signature(
            &key,
            &signature,
            b"modified payload",
            "1.2.3",
            "fixture"
        )
        .is_err()
    );
    assert!(
        fixture_update(&key, &signature)
            .verify(b"modified payload")
            .is_err()
    );
    assert!(fixture_update(&key, "").verify(PAYLOAD).is_err());
    let (other_key, _) = signed_fixture("version:1.2.3");
    assert!(
        fixture_update(&other_key, &signature)
            .verify(PAYLOAD)
            .is_err()
    );
}

#[test]
fn real_updater_rejects_unsigned_version_and_manifest_version_substitution() {
    let (key, signature) = signed_fixture("timestamp:0\tfile:fixture");
    assert!(matches!(
        fixture_update(&key, &signature).verify(PAYLOAD),
        Err(tauri_plugin_updater::Error::MissingSignedVersion)
    ));
    let (key, signature) = signed_fixture("timestamp:0\tfile:fixture\tversion:0.1.0");
    assert!(matches!(
        fixture_update(&key, &signature).verify(PAYLOAD),
        Err(tauri_plugin_updater::Error::SignedVersionMismatch { .. })
    ));
}

#[test]
fn real_updater_does_not_offer_a_downgrade_even_with_a_valid_signature() {
    let (key, signature) = signed_fixture("version:0.0.1");
    assert!(fixture_updater(&key).restore_update(serde_json::json!({
        "version":"0.0.1", "url":"https://example.invalid/never-downloaded", "signature":signature
    })).unwrap().is_none());
}

#[test]
fn generated_static_manifest_is_consumable_by_the_real_updater_for_every_planned_target() {
    generated_manifest_for_profile(giveaway_scout_release_tools::ReleaseProfile::AllPlatforms);
}

#[test]
fn windows_only_manifest_is_consumable_without_offering_experimental_platform_updates() {
    generated_manifest_for_profile(giveaway_scout_release_tools::ReleaseProfile::WindowsX64);
}

fn generated_manifest_for_profile(profile: giveaway_scout_release_tools::ReleaseProfile) {
    use giveaway_scout_release_tools::{Artifact, ReleaseInput, TARGETS, prepare};
    let temporary = tempfile::tempdir().unwrap();
    let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let public_key = STANDARD.encode(keys.pk.to_box().unwrap().into_string());
    let public_path = temporary.path().join("updater.pub");
    std::fs::write(&public_path, &public_key).unwrap();
    let mut artifacts = Vec::new();
    for &target in profile.targets() {
        let name = match target {
            "darwin-aarch64" => "fixture.app.tar.gz",
            "linux-aarch64" => "fixture-arm64.AppImage",
            "linux-x86_64" => "fixture-amd64.AppImage",
            "windows-aarch64" => "fixture_arm64-setup.exe",
            _ => "fixture_x64-setup.exe",
        };
        std::fs::write(temporary.path().join(name), PAYLOAD).unwrap();
        let signature = minisign::sign(
            Some(&keys.pk),
            &keys.sk,
            std::io::Cursor::new(PAYLOAD),
            Some(&format!("timestamp:0\tfile:{name}\tversion:1.2.3")),
            Some("ephemeral test fixture"),
        )
        .unwrap();
        let signature_name = format!("{name}.sig");
        std::fs::write(
            temporary.path().join(&signature_name),
            STANDARD.encode(signature.into_string()),
        )
        .unwrap();
        artifacts.push(Artifact {
            target: target.to_owned(),
            path: name.into(),
            signature_path: signature_name.into(),
            url: format!("https://example.invalid/never-downloaded/{name}"),
        });
    }
    let input = ReleaseInput {
        release_profile: profile,
        version: "1.2.3".to_owned(),
        previous_version: "1.2.2".to_owned(),
        pub_date: "2020-01-01T00:00:00Z".to_owned(),
        notes: "Plain release text".to_owned(),
        artifacts,
    };
    let input_path = temporary.path().join("release.json");
    std::fs::write(&input_path, serde_json::to_vec(&input).unwrap()).unwrap();
    let output = temporary.path().join("output");
    let report = prepare(&input_path, &public_path, &output).unwrap();
    assert_eq!(report.release_profile, profile);
    assert!(!report.public_release_ready && !report.native_acceptance_verified);
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("latest.json")).unwrap()).unwrap();
    let release: tauri_plugin_updater::RemoteRelease = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(release.version.to_string(), "1.2.3");
    for target in TARGETS {
        if !profile.targets().contains(&target) {
            assert!(release.download_url(target).is_err());
            assert!(
                fixture_updater_for_target(&public_key, Some(target))
                    .restore_update(raw.clone())
                    .is_err()
            );
            continue;
        }
        assert!(release.download_url(target).is_ok());
        let update = fixture_updater_for_target(&public_key, Some(target))
            .restore_update(raw.clone())
            .unwrap()
            .unwrap();
        update.verify(PAYLOAD).unwrap();
    }
}
