use runtime::config_ops::{config_get, config_path, config_set, config_unset, ConfigError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigAction {
    Get {
        key: Option<String>,
        effective: bool,
    },
    Set {
        key: String,
        value: String,
    },
    Unset {
        key: String,
    },
    Path,
}

#[must_use]
pub fn run_config(action: ConfigAction, json: bool) -> i32 {
    match run_config_inner(action, json) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            match err {
                ConfigError::UnknownKey(_) | ConfigError::BadValue { .. } => 2,
                ConfigError::Io(_) => 1,
            }
        }
    }
}

fn run_config_inner(action: ConfigAction, json: bool) -> Result<(), ConfigError> {
    match action {
        ConfigAction::Get { key, effective } => {
            let output = config_get(key.as_deref().unwrap_or(""), effective)?;
            if json || key.is_none() {
                println!("{output}");
            } else {
                let parsed: serde_json::Value = serde_json::from_str(&output).map_err(|err| {
                    ConfigError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
                })?;
                match parsed {
                    serde_json::Value::String(value) => println!("{value}"),
                    other => println!(
                        "{}",
                        serde_json::to_string(&other).map_err(|err| {
                            ConfigError::Io(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                err,
                            ))
                        })?
                    ),
                }
            }
            Ok(())
        }
        ConfigAction::Set { key, value } => config_set(&key, &value),
        ConfigAction::Unset { key } => config_unset(&key),
        ConfigAction::Path => {
            let path = config_path();
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&path.to_string_lossy().to_string()).map_err(|err| {
                        ConfigError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
                    })?
                );
            } else {
                println!("{}", path.display());
            }
            Ok(())
        }
    }
}
