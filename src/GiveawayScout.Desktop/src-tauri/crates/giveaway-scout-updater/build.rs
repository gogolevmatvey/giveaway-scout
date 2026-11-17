use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=GIVEAWAY_SCOUT_RELEASE_CONFIG");
    let declaration = match env::var_os("GIVEAWAY_SCOUT_RELEASE_CONFIG") {
        None => "pub const COMPILED_CONFIG: Option<UpdateConfig> = None;\n".to_owned(),
        Some(path) => {
            let path = PathBuf::from(path);
            assert!(
                path.is_absolute(),
                "Release configuration path must be absolute"
            );
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = fs::read(path).expect("Cannot read release configuration");
            assert!(bytes.len() <= 16_384, "Release configuration is too large");
            let root: serde_json::Value =
                serde_json::from_slice(&bytes).expect("Invalid release configuration JSON");
            let config = &root["plugins"]["updater"];
            let endpoint = "https://github.com/gogolevmatvey/giveaway-scout/releases/latest/download/latest.json";
            assert_eq!(
                config["endpoints"],
                serde_json::json!([endpoint]),
                "Unexpected release endpoint"
            );
            assert_eq!(
                config["requireSignedVersion"], true,
                "Signed versions must be required"
            );
            assert_eq!(
                config["allowDowngrades"], false,
                "Downgrades must be disabled"
            );
            for name in [
                "dangerousInsecureTransportProtocol",
                "dangerousAcceptInvalidCerts",
                "dangerousAcceptInvalidHostnames",
            ] {
                assert!(
                    config.get(name).is_none_or(|value| value == false),
                    "Unsafe update configuration"
                );
            }
            let public_key = config["pubkey"]
                .as_str()
                .filter(|key| !key.is_empty() && key.len() <= 4096)
                .expect("Missing update public key");
            format!(
                "pub const COMPILED_CONFIG: Option<UpdateConfig> = Some(UpdateConfig {{ endpoint: {endpoint:?}, public_key: {public_key:?} }});\n"
            )
        }
    };
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("release_config.rs"),
        declaration,
    )
    .unwrap();
}
