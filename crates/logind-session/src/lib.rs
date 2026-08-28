//! Fail-closed systemd-logind session resolve and inhibitor RAII.
//!
//! Session resolution order:
//! 1. `GetSessionByPID` (pid `0` = calling process)
//! 2. `GetSession($XDG_SESSION_ID)` when that variable is set and non-empty
//! 3. `ListSessions` scored for the calling uid
//!
//! The manager object path `/org/freedesktop/login1` is never a session path.

use std::env;
use std::ffi::OsString;
use std::fmt;

use zbus::zvariant::OwnedObjectPath;
use zbus::Connection;

mod proxies;

pub use proxies::{LogindManagerProxy, LogindSessionProxy};
pub use zbus::zvariant::OwnedFd;

/// logind Manager object path. Never a session.
pub const MANAGER_PATH: &str = "/org/freedesktop/login1";

/// Why a session or inhibitor could not be resolved.
#[derive(Debug)]
pub enum Error {
    Dbus(zbus::Error),
    MissingSessionId,
    EmptySessionId,
    ManagerPath,
    NoSession,
    NotGraphical { session_type: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dbus(e) => write!(f, "{e}"),
            Self::MissingSessionId => write!(f, "XDG_SESSION_ID is unset"),
            Self::EmptySessionId => write!(f, "XDG_SESSION_ID is set but empty"),
            Self::ManagerPath => {
                write!(
                    f,
                    "refusing to treat {MANAGER_PATH} as a session object path"
                )
            }
            Self::NoSession => write!(f, "no logind session could be resolved"),
            Self::NotGraphical { session_type } => {
                write!(f, "not a graphical session (type: {session_type})")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Dbus(e) => Some(e),
            _ => None,
        }
    }
}

impl From<zbus::Error> for Error {
    fn from(value: zbus::Error) -> Self {
        Self::Dbus(value)
    }
}

/// Resolved logind session. `path` is never the manager object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionRef {
    id: String,
    path: OwnedObjectPath,
}

impl SessionRef {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn path(&self) -> &OwnedObjectPath {
        &self.path
    }

    pub async fn proxy<'a>(&self, conn: &'a Connection) -> Result<LogindSessionProxy<'a>, Error> {
        Ok(LogindSessionProxy::builder(conn)
            .path(self.path.clone())?
            .build()
            .await?)
    }
}

/// Holding the fd keeps the logind inhibit active. Drop releases it.
pub struct Inhibitor {
    _fd: OwnedFd,
}

impl Inhibitor {
    pub async fn acquire(
        manager: &LogindManagerProxy<'_>,
        what: &str,
        who: &str,
        why: &str,
        mode: &str,
    ) -> Result<Self, Error> {
        let fd = manager.inhibit(what, who, why, mode).await?;
        Ok(Self { _fd: fd })
    }
}

/// `XDG_SESSION_ID` from the process environment. Empty is an error.
pub fn session_id_from_env() -> Result<String, Error> {
    session_id_from_os_var(env::var_os("XDG_SESSION_ID"))
}

/// `XDG_SESSION_ID` from an explicit value. Used by tests.
pub fn session_id_from_os_var(value: Option<OsString>) -> Result<String, Error> {
    let Some(value) = value else {
        return Err(Error::MissingSessionId);
    };
    if value.is_empty() {
        return Err(Error::EmptySessionId);
    }
    Ok(value.to_string_lossy().into_owned())
}

pub fn is_graphical_type(session_type: &str) -> bool {
    matches!(session_type, "wayland" | "x11" | "mir")
}

/// Score a ListSessions candidate. Higher wins.
///
/// class=user +1, graphical type +2, active +4 / online +1.
pub fn score_session(class: &str, session_type: &str, state: &str) -> i32 {
    let mut score = 0;
    if class == "user" {
        score += 1;
    }
    if is_graphical_type(session_type) {
        score += 2;
    }
    if state == "active" {
        score += 4;
    } else if state == "online" {
        score += 1;
    }
    score
}

pub fn reject_manager_path(path: &OwnedObjectPath) -> Result<(), Error> {
    if path.as_str() == MANAGER_PATH {
        return Err(Error::ManagerPath);
    }
    Ok(())
}

/// Resolve the calling process's session. Does not require a graphical type.
pub async fn resolve_session(conn: &Connection) -> Result<SessionRef, Error> {
    let manager = LogindManagerProxy::new(conn).await?;
    resolve_session_with(conn, &manager, 0).await
}

/// Resolve and require wayland/x11/mir.
pub async fn resolve_graphical_session(conn: &Connection) -> Result<SessionRef, Error> {
    let session = resolve_session(conn).await?;
    let proxy = session.proxy(conn).await?;
    let session_type = proxy.session_type().await?;
    if !is_graphical_type(&session_type) {
        return Err(Error::NotGraphical { session_type });
    }
    Ok(session)
}

pub async fn resolve_session_with(
    conn: &Connection,
    manager: &LogindManagerProxy<'_>,
    pid: u32,
) -> Result<SessionRef, Error> {
    if let Ok(path) = manager.get_session_by_pid(pid).await {
        if reject_manager_path(&path).is_ok() {
            return session_from_path(conn, path).await;
        }
    }

    if let Ok(id) = session_id_from_env() {
        if let Ok(path) = manager.get_session(&id).await {
            reject_manager_path(&path)?;
            return Ok(SessionRef { id, path });
        }
    }

    let uid = current_uid();
    let sessions = manager.list_sessions().await?;
    let mut best: Option<(i32, String, OwnedObjectPath)> = None;
    for (id, suid, _user, _seat, path) in sessions {
        if suid != uid {
            continue;
        }
        if reject_manager_path(&path).is_err() {
            continue;
        }
        let Ok(proxy) = LogindSessionProxy::builder(conn)
            .path(path.clone())?
            .build()
            .await
        else {
            continue;
        };
        let (Ok(state), Ok(class), Ok(stype)) = (
            proxy.state().await,
            proxy.class().await,
            proxy.session_type().await,
        ) else {
            continue;
        };
        let score = score_session(&class, &stype, &state);
        if best.as_ref().is_none_or(|(s, _, _)| score > *s) {
            best = Some((score, id, path));
        }
    }
    let Some((_, id, path)) = best else {
        return Err(Error::NoSession);
    };
    Ok(SessionRef { id, path })
}

async fn session_from_path(conn: &Connection, path: OwnedObjectPath) -> Result<SessionRef, Error> {
    reject_manager_path(&path)?;
    let proxy = LogindSessionProxy::builder(conn)
        .path(path.clone())?
        .build()
        .await?;
    let id = proxy.id().await?;
    if id.is_empty() {
        return Err(Error::EmptySessionId);
    }
    Ok(SessionRef { id, path })
}

fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_xdg_session_id_is_an_error() {
        assert!(matches!(
            session_id_from_os_var(None),
            Err(Error::MissingSessionId)
        ));
        assert!(matches!(
            session_id_from_os_var(Some(OsString::from(""))),
            Err(Error::EmptySessionId)
        ));
    }

    #[test]
    fn reads_xdg_session_id() {
        assert_eq!(
            session_id_from_os_var(Some(OsString::from("c2"))).unwrap(),
            "c2"
        );
    }

    #[test]
    fn manager_path_is_rejected() {
        let path = OwnedObjectPath::try_from(MANAGER_PATH).unwrap();
        assert!(matches!(
            reject_manager_path(&path),
            Err(Error::ManagerPath)
        ));
        assert!(path.as_str() == "/org/freedesktop/login1");
    }

    #[test]
    fn session_path_is_accepted() {
        let path = OwnedObjectPath::try_from("/org/freedesktop/login1/session/_32").unwrap();
        assert!(reject_manager_path(&path).is_ok());
    }

    #[test]
    fn scores_active_graphical_user_highest() {
        assert_eq!(score_session("user", "wayland", "active"), 7);
        assert_eq!(score_session("user", "x11", "online"), 4);
        assert_eq!(score_session("user", "tty", "active"), 5);
        assert_eq!(score_session("manager", "unspecified", "closing"), 0);
    }

    #[test]
    fn graphical_types_include_mir() {
        assert!(is_graphical_type("wayland"));
        assert!(is_graphical_type("x11"));
        assert!(is_graphical_type("mir"));
        assert!(!is_graphical_type("tty"));
    }
}
