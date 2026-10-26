use std::{ffi::OsString, path::PathBuf};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new("--help")) {
        println!(
            "giveaway-scout-release prepare --input release.json --public-key updater.pub --output NEW_DIRECTORY\nOffline verification/staging only. No private key, network, installation or publication."
        );
        return;
    }
    let result = (|| {
        if first != Some(OsString::from("prepare")) {
            return Err("Expected prepare command".to_owned());
        }
        let mut paths: [Option<PathBuf>; 3] = [None, None, None];
        while let Some(flag) = args.next() {
            let index = match flag.to_str() {
                Some("--input") => 0,
                Some("--public-key") => 1,
                Some("--output") => 2,
                _ => return Err("Unknown argument".to_owned()),
            };
            if paths[index].is_some() {
                return Err("Duplicate argument".to_owned());
            }
            paths[index] = Some(PathBuf::from(args.next().ok_or("Missing argument value")?));
        }
        let [Some(input), Some(public_key), Some(output)] = paths else {
            return Err("All three paths are required".to_owned());
        };
        giveaway_scout_release_tools::prepare(&input, &public_key, &output)?;
        println!(
            "Updater signatures verified; local snapshot prepared. Public release readiness is NOT established. Nothing was published or installed."
        );
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        eprintln!("Release preparation failed: {error}");
        std::process::exit(1);
    }
}
