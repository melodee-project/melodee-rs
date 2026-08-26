//! Shared CLI infrastructure: database access and operator-input validation.

use std::{env, io::Read};

use sqlx::PgPool;

/// The database URL comes from exactly one explicit key
/// (`MELODEE__DATABASE__URL`). It is never printed because it may contain a
/// password.
pub(crate) fn database_url() -> String {
    match env::var(melodee_persistence::ENV_DATABASE_URL) {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!(
                "error: {} is not set (example: postgres://user:pass@host:5432/melodee)",
                melodee_persistence::ENV_DATABASE_URL
            );
            std::process::exit(2);
        }
    }
}

pub(crate) async fn pool() -> PgPool {
    PgPool::connect(&database_url()).await.unwrap_or_else(|e| {
        eprintln!("error: cannot connect to database: {e}");
        std::process::exit(2);
    })
}

pub(crate) fn parse_nonblank(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        Err("value must not be blank".to_owned())
    } else {
        Ok(value.to_owned())
    }
}

/// Read an operator-supplied regular file through a held no-follow parent
/// descriptor and retain at most `limit + 1` bytes. The extra byte lets callers
/// distinguish an exactly-at-limit document from an oversized one without
/// allocating an unbounded buffer. Unsupported platforms fail closed because
/// these CLI inputs can drive database or script operations.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub(crate) fn read_bounded_file(path: &std::path::Path, limit: usize) -> std::io::Result<Vec<u8>> {
    use std::{ffi::OsString, os::unix::ffi::OsStrExt, path::Component};

    use rustix::fs::{CWD, FileType, Mode, OFlags, fstat, openat};

    let input = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    let mut components = Vec::<OsString>::new();
    for component in input.components() {
        match component {
            Component::RootDir => components.clear(),
            Component::Normal(name) => components.push(name.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop();
            }
            Component::Prefix(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "input path has an unsupported prefix",
                ));
            }
        }
    }
    let mut absolute = std::path::PathBuf::from("/");
    for component in components {
        absolute.push(component);
    }
    let parent_path = absolute.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "input path has no parent")
    })?;
    let name = absolute.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "input path has no file name",
        )
    })?;
    let mut parent = openat(
        CWD,
        b"/".as_slice(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    for component in parent_path.components() {
        let Component::Normal(component) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "input parent contains traversal",
            ));
        };
        parent = openat(
            &parent,
            component.as_bytes(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
    }
    let file = openat(
        &parent,
        name.as_bytes(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let stat = fstat(&file).map_err(std::io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "input is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::from(file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
pub(crate) fn read_bounded_file(
    _path: &std::path::Path,
    _limit: usize,
) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "race-resistant CLI file reads are unsupported on this platform",
    ))
}
