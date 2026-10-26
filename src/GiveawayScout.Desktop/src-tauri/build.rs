fn main() {
    if std::env::var_os("CARGO_FEATURE_UI_TESTS").is_some()
        && std::env::var("PROFILE").as_deref() != Ok("debug")
    {
        panic!("ui-tests is forbidden outside the debug profile");
    }
    tauri_build::build();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // Tauri embeds resources for app binaries, not integration-test EXEs.
        // The mock-runtime fixture links TaskDialogIndirect from Common Controls
        // v6; without an activation manifest Windows loads v5 and cannot start it.
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-tests=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
