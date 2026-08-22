use std::{
    env,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::Mutex,
};

use thiserror::Error;
use tracing_subscriber::{
    EnvFilter,
    fmt::writer::{BoxMakeWriter, MakeWriterExt},
};

pub(crate) const ENV_LOGGING_DIRECTORY: &str = "MELODEE__LOGGING__DIRECTORY";
pub(crate) const ENV_LOGGING_LEVEL: &str = "MELODEE__LOGGING__LEVEL";

const DEFAULT_LOGGING_LEVEL: &str = "info";
const LOG_FILE_NAME: &str = "melodee.log";

#[derive(Debug, Error)]
pub(crate) enum LoggingError {
    #[error("{ENV_LOGGING_LEVEL} must be one of trace, debug, info, warn, or error; got {value:?}")]
    InvalidLevel { value: String },
    #[error("unable to create logging directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unable to open log file {path}: {source}")]
    OpenFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unable to install logging subscriber: {0}")]
    InstallSubscriber(String),
}

pub(crate) fn init() -> Result<(), LoggingError> {
    let configured_level = env::var(ENV_LOGGING_LEVEL).ok();
    let (filter, filter_description) = match configured_level.as_deref() {
        Some(level) => {
            let directive = level_directive(level)?;
            let filter = EnvFilter::try_new(&directive).map_err(|error| {
                LoggingError::InstallSubscriber(format!("invalid generated filter: {error}"))
            })?;
            (filter, directive)
        }
        None => match EnvFilter::try_from_default_env() {
            Ok(filter) => {
                let description = env::var("RUST_LOG").unwrap_or_else(|_| filter.to_string());
                (filter, description)
            }
            Err(_) => {
                let directive = level_directive(DEFAULT_LOGGING_LEVEL)?;
                let filter = EnvFilter::try_new(&directive).map_err(|error| {
                    LoggingError::InstallSubscriber(format!("invalid default filter: {error}"))
                })?;
                (filter, directive)
            }
        },
    };

    let directory = env::var_os(ENV_LOGGING_DIRECTORY)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let (writer, log_file) = match directory {
        Some(directory) => {
            let (file, path) = open_log_file(&directory)?;
            (
                BoxMakeWriter::new(std::io::stderr.and(Mutex::new(file))),
                Some(path),
            )
        }
        None => (BoxMakeWriter::new(std::io::stderr), None),
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_ansi(false)
        .with_writer(writer)
        .try_init()
        .map_err(|error| LoggingError::InstallSubscriber(error.to_string()))?;

    tracing::info!(
        filter = %filter_description,
        log_file = log_file.as_ref().map(|path| path.display().to_string()),
        "logging initialized"
    );
    Ok(())
}

fn level_directive(value: &str) -> Result<String, LoggingError> {
    let level = value.trim().to_ascii_lowercase();
    if !matches!(
        level.as_str(),
        "trace" | "debug" | "info" | "warn" | "error"
    ) {
        return Err(LoggingError::InvalidLevel {
            value: value.to_owned(),
        });
    }
    Ok(format!("melodee={level},tower_http=info"))
}

fn open_log_file(directory: &Path) -> Result<(File, PathBuf), LoggingError> {
    fs::create_dir_all(directory).map_err(|source| LoggingError::CreateDirectory {
        path: directory.to_path_buf(),
        source,
    })?;
    let path = directory.join(LOG_FILE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|source| LoggingError::OpenFile {
            path: path.clone(),
            source,
        })?;
    Ok((file, path))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn logging_level_accepts_documented_values_case_insensitively() {
        assert_eq!(
            level_directive("debug").unwrap(),
            "melodee=debug,tower_http=info"
        );
        assert_eq!(
            level_directive(" WARN ").unwrap(),
            "melodee=warn,tower_http=info"
        );
        assert!(level_directive("verbose").is_err());
        assert!(level_directive("").is_err());
    }

    #[test]
    fn log_file_is_created_and_appended() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("nested").join("logs");

        let (mut first, path) = open_log_file(&directory).unwrap();
        writeln!(first, "first").unwrap();
        drop(first);

        let (mut second, second_path) = open_log_file(&directory).unwrap();
        writeln!(second, "second").unwrap();
        drop(second);

        assert_eq!(path, second_path);
        assert_eq!(fs::read_to_string(path).unwrap(), "first\nsecond\n");
    }
}
