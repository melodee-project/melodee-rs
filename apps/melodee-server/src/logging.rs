use std::{
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use thiserror::Error;
use time::{Date, OffsetDateTime};
use tracing_subscriber::{
    EnvFilter,
    fmt::writer::{BoxMakeWriter, MakeWriterExt},
};

pub(crate) const ENV_LOGGING_DIRECTORY: &str = "MELODEE__LOGGING__DIRECTORY";
pub(crate) const ENV_LOGGING_LEVEL: &str = "MELODEE__LOGGING__LEVEL";

const DEFAULT_LOGGING_LEVEL: &str = "info";

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
            let (file, path) = DailyLogWriter::new(&directory)?;
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
    Ok(format!("melodee={level},tower_http={level}"))
}

struct DailyLogWriter {
    directory: PathBuf,
    date: Date,
    file: File,
}

impl DailyLogWriter {
    fn new(directory: &Path) -> Result<(Self, PathBuf), LoggingError> {
        Self::new_for_date(directory, OffsetDateTime::now_utc().date())
    }

    fn new_for_date(directory: &Path, date: Date) -> Result<(Self, PathBuf), LoggingError> {
        fs::create_dir_all(directory).map_err(|source| LoggingError::CreateDirectory {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = daily_log_path(directory, date);
        let file = open_append(&path).map_err(|source| LoggingError::OpenFile {
            path: path.clone(),
            source,
        })?;
        Ok((
            Self {
                directory: directory.to_path_buf(),
                date,
                file,
            },
            path,
        ))
    }

    fn write_for_date(&mut self, date: Date, buffer: &[u8]) -> std::io::Result<usize> {
        if self.date != date {
            let path = daily_log_path(&self.directory, date);
            let file = open_append(&path)?;
            self.file = file;
            self.date = date;
        }
        self.file.write_all(buffer)?;
        Ok(buffer.len())
    }
}

impl Write for DailyLogWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.write_for_date(OffsetDateTime::now_utc().date(), buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn daily_log_path(directory: &Path, date: Date) -> PathBuf {
    directory.join(format!("{date}.log"))
}

fn open_append(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
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
            "melodee=debug,tower_http=debug"
        );
        assert_eq!(
            level_directive(" WARN ").unwrap(),
            "melodee=warn,tower_http=warn"
        );
        assert!(level_directive("verbose").is_err());
        assert!(level_directive("").is_err());
    }

    #[test]
    fn daily_log_file_is_created_and_appended() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("nested").join("logs");
        let date = Date::from_calendar_date(2026, time::Month::August, 22).unwrap();

        let (mut first, path) = DailyLogWriter::new_for_date(&directory, date).unwrap();
        writeln!(first, "first").unwrap();
        drop(first);

        let (mut second, second_path) = DailyLogWriter::new_for_date(&directory, date).unwrap();
        writeln!(second, "second").unwrap();
        drop(second);

        assert_eq!(path, second_path);
        assert_eq!(path.file_name().unwrap(), "2026-08-22.log");
        assert_eq!(fs::read_to_string(path).unwrap(), "first\nsecond\n");
    }

    #[test]
    fn daily_log_file_rotates_when_the_utc_date_changes() {
        let temp = tempfile::tempdir().unwrap();
        let first_date = Date::from_calendar_date(2026, time::Month::August, 22).unwrap();
        let second_date = Date::from_calendar_date(2026, time::Month::August, 23).unwrap();
        let (mut writer, first_path) =
            DailyLogWriter::new_for_date(temp.path(), first_date).unwrap();

        writer.write_for_date(first_date, b"first\n").unwrap();
        writer.write_for_date(second_date, b"second\n").unwrap();
        writer.flush().unwrap();

        let second_path = daily_log_path(temp.path(), second_date);
        assert_eq!(fs::read_to_string(first_path).unwrap(), "first\n");
        assert_eq!(fs::read_to_string(second_path).unwrap(), "second\n");
    }
}
