use std::path::PathBuf;

use giveaway_scout_core::{GameStore, GiveawaySource, InitialSyncMode};

pub const HELP: &str = "Giveaway Scout CLI

Usage: giveaway-scout-cli [--json] [--data-dir PATH] COMMAND

Commands:
  check [--initial-sync silent|notify-current]   Check enabled sources once
  watch [--initial-sync silent|notify-current]   Monitor until Ctrl+C
  history [--search TEXT] [--source SOURCE] [--store STORE] [--unread]
  read ID                                      Show a record and mark it read
  read --all                                   Mark the entire history read
  open ID                                      Open its URL and mark it read
  config show                                  Show all settings
  config set KEY VALUE                         Update a setting
  update check                                 Check for a new release
  update install                               Verify and launch the signed setup

Global options:
  --json             JSONL for check/watch; JSON for other commands
  --data-dir PATH    Use a separate application profile
  -h, --help         Show help
  -V, --version      Show version

Examples:
  giveaway-scout-cli watch
  giveaway-scout-cli --json check
  giveaway-scout-cli history --unread
  giveaway-scout-cli read 123e4567-e89b-42d3-a456-426614174000
  giveaway-scout-cli open 123e4567-e89b-42d3-a456-426614174000
  giveaway-scout-cli config set sourceCheckIntervals.giveeClub 30
  giveaway-scout-cli config set autostartTarget cli
  giveaway-scout-cli config set startAtLogin true

Copy the full record ID from history or a console notification.
read marks a record after successful output; open after the browser launcher succeeds.
read --all affects the entire history and accepts no filters.
Source names: freeSteamKeys, giveeClub, gamesForFarm, zeros
Stores: steam, epicGames, gog, unknown
Current items are silently remembered on the first run unless notify-current is selected.
GUI and CLI use the same profile in turn; quit the other program first.
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct HistoryFilters {
    pub search: String,
    pub source: Option<GiveawaySource>,
    pub store: Option<GameStore>,
    pub unread: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Check(Option<InitialSyncMode>),
    Watch(Option<InitialSyncMode>),
    History(HistoryFilters),
    Read(String),
    ReadAll,
    Open(String),
    ConfigShow,
    ConfigSet {
        key: String,
        value: String,
    },
    UpdateCheck,
    UpdateInstall,
    MaintenanceCheck,
    Maintenance {
        components: u32,
        install_dir: PathBuf,
        remove: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Arguments {
    pub json: bool,
    pub data_dir: Option<PathBuf>,
    pub command: Command,
}

fn take_value(args: &[String], index: &mut usize, name: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
}

pub fn parse(input: impl IntoIterator<Item = String>) -> Result<Arguments, String> {
    let input: Vec<_> = input.into_iter().collect();
    let mut json = false;
    let mut data_dir = None;
    let mut args = Vec::new();
    let mut index = 0;
    // Config values and installer paths are positional data, not global flags.
    while index < input.len() {
        if args.first().is_some_and(|value: &String| value == "config")
            && args.get(1).is_some_and(|value| value == "set")
            && args.len() == 3
        {
            args.push(input[index].clone());
            index += 1;
            continue;
        }
        match input[index].as_str() {
            "--json" => json = true,
            "--data-dir" => {
                data_dir = Some(PathBuf::from(take_value(&input, &mut index, "--data-dir")?))
            }
            value
                if matches!(
                    value,
                    "--search"
                        | "--source"
                        | "--store"
                        | "--install-dir"
                        | "--components"
                        | "--initial-sync"
                ) =>
            {
                args.push(value.to_owned());
                args.push(take_value(&input, &mut index, value)?);
            }
            value => args.push(value.to_owned()),
        }
        index += 1;
    }
    if args.iter().enumerate().any(|(index, value)| {
        matches!(value.as_str(), "--help" | "-h")
            && !(index == 3 && args.first().is_some_and(|value| value == "config"))
            && !index.checked_sub(1).is_some_and(|previous| {
                matches!(
                    args[previous].as_str(),
                    "--search"
                        | "--source"
                        | "--store"
                        | "--install-dir"
                        | "--components"
                        | "--initial-sync"
                )
            })
    }) && args.first().is_none_or(|value| value != "__maintenance")
    {
        return Ok(Arguments {
            json,
            data_dir,
            command: Command::Help,
        });
    }
    let command = match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "help") if args.len() <= 1 => Command::Help,
        Some("--version" | "-V") if args.len() == 1 => Command::Version,
        Some("check" | "watch") => {
            let initial = match args.as_slice() {
                [_] => None,
                [_, flag, mode] if flag == "--initial-sync" => Some(match mode.as_str() {
                    "silent" => InitialSyncMode::Silent,
                    "notify-current" => InitialSyncMode::NotifyCurrent,
                    _ => return Err("--initial-sync must be silent or notify-current".to_owned()),
                }),
                _ => return Err("Unexpected check/watch arguments; use --help".to_owned()),
            };
            if args[0] == "check" {
                Command::Check(initial)
            } else {
                Command::Watch(initial)
            }
        }
        Some("history") => {
            let mut filters = HistoryFilters::default();
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--unread" => filters.unread = true,
                    "--search" => filters.search = take_value(&args, &mut i, "--search")?,
                    "--source" => {
                        let value = take_value(&args, &mut i, "--source")?;
                        filters.source = Some(
                            GiveawaySource::ALL
                                .into_iter()
                                .find(|source| source.storage_name().eq_ignore_ascii_case(&value))
                                .ok_or_else(|| format!("Unknown source: {value}"))?,
                        );
                    }
                    "--store" => {
                        let value = take_value(&args, &mut i, "--store")?;
                        filters.store = Some(match value.as_str() {
                            "steam" => GameStore::Steam,
                            "epicGames" => GameStore::EpicGames,
                            "gog" => GameStore::Gog,
                            "unknown" => GameStore::Unknown,
                            _ => return Err(format!("Unknown store: {value}")),
                        });
                    }
                    value => return Err(format!("Unknown history option: {value}")),
                }
                i += 1;
            }
            Command::History(filters)
        }
        Some("read") => match args.as_slice() {
            [_, flag] if flag == "--all" => Command::ReadAll,
            [_, id] if !id.is_empty() && !id.starts_with('-') => Command::Read(id.clone()),
            _ => return Err("Use read ID or read --all".to_owned()),
        },
        Some("open") => match args.as_slice() {
            [_, id] if !id.is_empty() && !id.starts_with('-') => Command::Open(id.clone()),
            _ => return Err("Use open ID".to_owned()),
        },
        Some("config") => match args.as_slice() {
            [_, action] if action == "show" => Command::ConfigShow,
            [_, action, key, value] if action == "set" => Command::ConfigSet {
                key: key.clone(),
                value: value.clone(),
            },
            _ => return Err("Use config show or config set KEY VALUE".to_owned()),
        },
        Some("update") => match args.as_slice() {
            [_, action] if action == "check" => Command::UpdateCheck,
            [_, action] if action == "install" => Command::UpdateInstall,
            _ => return Err("Use update check or update install".to_owned()),
        },
        Some("__maintenance") => {
            if args.len() == 2 && args[1] == "check-profile" {
                return Ok(Arguments {
                    json,
                    data_dir,
                    command: Command::MaintenanceCheck,
                });
            }
            if args.get(1).map(String::as_str) != Some("sync-autostart") {
                return Err("Invalid maintenance operation".to_owned());
            }
            let mut components = None;
            let mut install_dir = None;
            let mut remove = false;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--components" => {
                        let mask = take_value(&args, &mut i, "--components")?
                            .parse::<u32>()
                            .map_err(|_| "Invalid component mask".to_owned())?;
                        components = Some(mask);
                    }
                    "--install-dir" => {
                        install_dir =
                            Some(PathBuf::from(take_value(&args, &mut i, "--install-dir")?))
                    }
                    "--remove" => remove = true,
                    _ => return Err("Unexpected maintenance argument".to_owned()),
                }
                i += 1;
            }
            let components = components.ok_or_else(|| "Missing component mask".to_owned())?;
            if components > 3 || (!remove && components == 0) {
                return Err("Invalid component mask".to_owned());
            }
            Command::Maintenance {
                components,
                install_dir: install_dir
                    .ok_or_else(|| "Missing installation directory".to_owned())?,
                remove,
            }
        }
        _ => return Err("Unknown command; use --help".to_owned()),
    };
    Ok(Arguments {
        json,
        data_dir,
        command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &[&str]) -> Result<Arguments, String> {
        parse(s.iter().map(|value| (*value).to_owned()))
    }

    #[test]
    fn handles_global_options_and_modes() {
        let parsed = args(&[
            "watch",
            "--json",
            "--data-dir",
            "C:\\test profile",
            "--initial-sync",
            "notify-current",
        ])
        .unwrap();
        assert!(parsed.json);
        assert_eq!(
            parsed.command,
            Command::Watch(Some(InitialSyncMode::NotifyCurrent))
        );
        assert_eq!(parsed.data_dir, Some(PathBuf::from("C:\\test profile")));
        assert!(args(&["check", "--initial-sync", "anything"]).is_err());
        assert!(args(&["update", "install", "--url", "https://example.invalid"]).is_err());
    }

    #[test]
    fn validates_read_and_open_without_accepting_history_filters() {
        assert_eq!(
            args(&["read", "giveeClub:42", "--json"]).unwrap().command,
            Command::Read("giveeClub:42".into())
        );
        assert_eq!(args(&["read", "--all"]).unwrap().command, Command::ReadAll);
        assert_eq!(
            args(&["open", "giveeClub:42"]).unwrap().command,
            Command::Open("giveeClub:42".into())
        );
        for input in [
            vec!["read"],
            vec!["open"],
            vec!["read", ""],
            vec!["open", "--all"],
            vec!["read", "--all", "--unread"],
            vec!["read", "42", "extra"],
            vec!["open", "42", "--source", "giveeClub"],
        ] {
            assert!(args(&input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn preserves_config_values_and_validates_filters() {
        assert_eq!(
            args(&["config", "set", "logDirectory", "C:\\space dir"])
                .unwrap()
                .command,
            Command::ConfigSet {
                key: "logDirectory".into(),
                value: "C:\\space dir".into()
            }
        );
        assert!(
            args(&[
                "history",
                "--source",
                "giveeClub",
                "--store",
                "steam",
                "--unread"
            ])
            .is_ok()
        );
        assert!(args(&["history", "--store", "invalid"]).is_err());
        assert!(args(&["--data-dir"]).is_err());
        assert!(
            args(&["config", "set", "languageCode", "ru", "--json"])
                .unwrap()
                .json
        );
        assert_eq!(
            args(&["history", "--search", "--json"]).unwrap().command,
            Command::History(HistoryFilters {
                search: "--json".into(),
                ..HistoryFilters::default()
            })
        );
        assert_eq!(
            args(&["history", "--search", "--help"]).unwrap().command,
            Command::History(HistoryFilters {
                search: "--help".into(),
                ..HistoryFilters::default()
            })
        );
    }
}
