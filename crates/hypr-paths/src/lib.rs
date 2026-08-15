//! Fail-closed XDG config, data, and runtime paths.
//!
//! - config: `$XDG_CONFIG_HOME` if set and absolute, else `$HOME/.config`
//! - data: `$XDG_DATA_HOME` if set and absolute, else `$HOME/.local/share`
//! - runtime: `$XDG_RUNTIME_DIR` must be set and absolute
//!
//! There is no `/run/user/<uid>` fallback, no `/tmp` fallback, and no `~`
//! expansion. Relative or empty XDG values are errors.

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// Resolved XDG base directories for one process environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseDirs {
    config_home: PathBuf,
    data_home: PathBuf,
    runtime_dir: PathBuf,
}

/// Why base directories could not be resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    MissingHome,
    MissingRuntimeDir,
    Relative { var: &'static str, value: PathBuf },
    Empty { var: &'static str },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHome => {
                write!(f, "HOME is unset or empty and an XDG fallback requires it")
            }
            Self::MissingRuntimeDir => {
                write!(f, "XDG_RUNTIME_DIR is unset or empty")
            }
            Self::Relative { var, value } => {
                write!(f, "{var} must be an absolute path, got {}", value.display())
            }
            Self::Empty { var } => write!(f, "{var} is set but empty"),
        }
    }
}

impl std::error::Error for Error {}

impl BaseDirs {
    /// Resolve from the current process environment.
    pub fn from_env() -> Result<Self, Error> {
        Self::from_os_vars(
            env::var_os("HOME"),
            env::var_os("XDG_CONFIG_HOME"),
            env::var_os("XDG_DATA_HOME"),
            env::var_os("XDG_RUNTIME_DIR"),
        )
    }

    /// Resolve from explicit variables. Used by tests; production code calls
    /// [`from_env`].
    pub fn from_os_vars(
        home: Option<OsString>,
        config_home: Option<OsString>,
        data_home: Option<OsString>,
        runtime_dir: Option<OsString>,
    ) -> Result<Self, Error> {
        let home = parse_home(home)?;
        Ok(Self {
            config_home: xdg_or_fallback(
                "XDG_CONFIG_HOME",
                config_home,
                home.as_deref(),
                ".config",
            )?,
            data_home: xdg_or_fallback(
                "XDG_DATA_HOME",
                data_home,
                home.as_deref(),
                ".local/share",
            )?,
            runtime_dir: required_absolute("XDG_RUNTIME_DIR", runtime_dir)?,
        })
    }

    pub fn config_home(&self) -> &Path {
        &self.config_home
    }

    pub fn data_home(&self) -> &Path {
        &self.data_home
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn config_dir(&self, app: &str) -> PathBuf {
        self.config_home.join(app)
    }

    pub fn data_dir(&self, app: &str) -> PathBuf {
        self.data_home.join(app)
    }

    pub fn runtime_path(&self, name: &str) -> PathBuf {
        self.runtime_dir.join(name)
    }
}

fn parse_home(home: Option<OsString>) -> Result<Option<PathBuf>, Error> {
    let Some(home) = home else {
        return Ok(None);
    };
    if home.is_empty() {
        return Err(Error::MissingHome);
    }
    let path = PathBuf::from(home);
    if path == Path::new("~") || !path.is_absolute() {
        return Err(Error::MissingHome);
    }
    Ok(Some(path))
}

fn xdg_or_fallback(
    var: &'static str,
    value: Option<OsString>,
    home: Option<&Path>,
    fallback: &str,
) -> Result<PathBuf, Error> {
    match value {
        None => {
            let home = home.ok_or(Error::MissingHome)?;
            Ok(home.join(fallback))
        }
        Some(value) if value.is_empty() => Err(Error::Empty { var }),
        Some(value) => {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(Error::Relative { var, value: path });
            }
            Ok(path)
        }
    }
}

fn required_absolute(var: &'static str, value: Option<OsString>) -> Result<PathBuf, Error> {
    let Some(value) = value else {
        return Err(Error::MissingRuntimeDir);
    };
    if value.is_empty() {
        return Err(Error::MissingRuntimeDir);
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(Error::Relative { var, value: path });
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(
        home: Option<&str>,
        config: Option<&str>,
        data: Option<&str>,
        runtime: Option<&str>,
    ) -> Result<BaseDirs, Error> {
        BaseDirs::from_os_vars(
            home.map(OsString::from),
            config.map(OsString::from),
            data.map(OsString::from),
            runtime.map(OsString::from),
        )
    }

    #[test]
    fn uses_xdg_when_absolute() {
        let got = dirs(
            Some("/home/mason"),
            Some("/custom/config"),
            Some("/custom/data"),
            Some("/run/user/1000"),
        )
        .unwrap();
        assert_eq!(got.config_home(), Path::new("/custom/config"));
        assert_eq!(got.data_home(), Path::new("/custom/data"));
        assert_eq!(got.runtime_dir(), Path::new("/run/user/1000"));
        assert_eq!(
            got.config_dir("hyprstate"),
            Path::new("/custom/config/hyprstate")
        );
        assert_eq!(
            got.runtime_path("hyprstate-telemetry.sock"),
            Path::new("/run/user/1000/hyprstate-telemetry.sock")
        );
    }

    #[test]
    fn falls_back_config_and_data_to_home() {
        let got = dirs(Some("/home/mason"), None, None, Some("/run/user/1000")).unwrap();
        assert_eq!(got.config_home(), Path::new("/home/mason/.config"));
        assert_eq!(got.data_home(), Path::new("/home/mason/.local/share"));
        assert_eq!(got.data_dir("lmtt"), Path::new("/home/mason/.local/share/lmtt"));
    }

    #[test]
    fn missing_runtime_is_an_error() {
        assert_eq!(
            dirs(Some("/home/mason"), None, None, None).unwrap_err(),
            Error::MissingRuntimeDir
        );
        assert_eq!(
            dirs(Some("/home/mason"), None, None, Some("")).unwrap_err(),
            Error::MissingRuntimeDir
        );
    }

    #[test]
    fn does_not_synthesize_run_user() {
        let err = dirs(Some("/home/mason"), None, None, None).unwrap_err();
        assert_eq!(err, Error::MissingRuntimeDir);
        assert!(!err.to_string().contains("/run/user/"));
    }

    #[test]
    fn rejects_relative_xdg() {
        assert!(matches!(
            dirs(Some("/home/mason"), Some("relative"), None, Some("/run/user/1")),
            Err(Error::Relative {
                var: "XDG_CONFIG_HOME",
                ..
            })
        ));
        assert!(matches!(
            dirs(Some("/home/mason"), None, None, Some("run/user/1")),
            Err(Error::Relative {
                var: "XDG_RUNTIME_DIR",
                ..
            })
        ));
        assert!(matches!(
            dirs(Some("/home/mason"), Some("~/.config"), None, Some("/run/user/1")),
            Err(Error::Relative {
                var: "XDG_CONFIG_HOME",
                ..
            })
        ));
    }

    #[test]
    fn rejects_tilde_home() {
        assert_eq!(
            dirs(Some("~"), None, None, Some("/run/user/1")).unwrap_err(),
            Error::MissingHome
        );
    }

    #[test]
    fn missing_home_without_xdg_overrides_fails() {
        assert_eq!(
            dirs(None, None, None, Some("/run/user/1")).unwrap_err(),
            Error::MissingHome
        );
    }

    #[test]
    fn missing_home_is_ok_when_xdg_config_and_data_are_set() {
        let got = dirs(
            None,
            Some("/etc/xdg-override"),
            Some("/var/lib/data"),
            Some("/run/user/1"),
        )
        .unwrap();
        assert_eq!(got.config_home(), Path::new("/etc/xdg-override"));
        assert_eq!(got.data_home(), Path::new("/var/lib/data"));
    }

    #[test]
    fn empty_xdg_config_is_an_error() {
        assert_eq!(
            dirs(Some("/home/mason"), Some(""), None, Some("/run/user/1")).unwrap_err(),
            Error::Empty {
                var: "XDG_CONFIG_HOME"
            }
        );
    }
}
