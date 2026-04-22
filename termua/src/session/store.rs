use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::settings::TerminalBackend;

#[cfg(unix)]
fn chmod_private_dir(path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod directory {path:?}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn chmod_private_dir(_path: &std::path::Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn chmod_private_file(path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod file {path:?}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn chmod_private_file(_path: &std::path::Path) -> anyhow::Result<()> {
    Ok(())
}

fn ensure_private_sqlite_files(db_path: &std::path::Path) -> anyhow::Result<()> {
    // Main db.
    if db_path.exists() {
        chmod_private_file(db_path)?;
    }

    // Best-effort: these files may appear depending on SQLite journaling mode.
    let mut maybe_sidecars = Vec::new();
    if let Some(name) = db_path.file_name().and_then(|n| n.to_str())
        && let Some(parent) = db_path.parent()
    {
        maybe_sidecars.push(parent.join(format!("{name}-wal")));
        maybe_sidecars.push(parent.join(format!("{name}-shm")));
        maybe_sidecars.push(parent.join(format!("{name}-journal")));
    }

    for path in maybe_sidecars {
        if path.exists() {
            let _ = chmod_private_file(&path);
        }
    }

    Ok(())
}

#[derive(Debug)]
struct ParseError(String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionType {
    Local,
    Ssh,
    Serial,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SshAuthType {
    Password,
    Config,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SerialParity {
    None,
    Even,
    Odd,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SerialStopBits {
    One,
    Two,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SerialFlowControl {
    None,
    Software,
    Hardware,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SshProxyMode {
    Inherit,
    Disabled,
    Command,
    JumpServer,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshProxyEnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshJumpHop {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionEnvVar {
    pub name: String,
    pub value: String,
}

const SESSION_ENV_TERM: &str = "TERM";
const SESSION_ENV_COLORTERM: &str = "COLORTERM";
const SESSION_ENV_CHARSET: &str = "CHARSET";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    pub id: i64,
    pub protocol: SessionType,
    pub group_path: String,
    pub label: String,

    pub backend: TerminalBackend,
    pub env: Option<Vec<SessionEnvVar>>,

    // Local
    pub shell_program: Option<String>,

    // SSH
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_auth_type: Option<SshAuthType>,
    pub ssh_user: Option<String>,
    pub ssh_credential_username: Option<String>,
    pub ssh_password: Option<String>,

    pub ssh_tcp_nodelay: bool,
    pub ssh_tcp_keepalive: bool,

    pub ssh_proxy_mode: Option<SshProxyMode>,
    pub ssh_proxy_command: Option<String>,
    pub ssh_proxy_workdir: Option<String>,
    pub ssh_proxy_env: Option<Vec<SshProxyEnvVar>>,
    pub ssh_proxy_jump: Option<Vec<SshJumpHop>>,

    // Serial
    pub serial_port: Option<String>,
    pub serial_baud: Option<u32>,
    pub serial_data_bits: Option<u8>,
    pub serial_parity: Option<SerialParity>,
    pub serial_stop_bits: Option<SerialStopBits>,
    pub serial_flow_control: Option<SerialFlowControl>,
}

impl Session {
    pub fn term(&self) -> &str {
        session_env_value(self.env.as_deref(), SESSION_ENV_TERM).unwrap_or("xterm-256color")
    }

    pub fn colorterm(&self) -> Option<&str> {
        session_env_value(self.env.as_deref(), SESSION_ENV_COLORTERM)
    }

    pub fn charset(&self) -> &str {
        session_env_value(self.env.as_deref(), SESSION_ENV_CHARSET).unwrap_or("UTF-8")
    }
}

fn session_env_value<'a>(env: Option<&'a [SessionEnvVar]>, name: &str) -> Option<&'a str> {
    env.and_then(|vars| {
        vars.iter()
            .find(|var| var.name == name)
            .map(|var| var.value.as_str())
    })
}

fn merge_terminal_fields_into_env(
    term: &str,
    colorterm: Option<&str>,
    charset: &str,
    env: Vec<SessionEnvVar>,
) -> Vec<SessionEnvVar> {
    let mut merged = Vec::new();
    upsert_session_env_var(&mut merged, SESSION_ENV_TERM, term.to_string());
    if let Some(colorterm) = colorterm.filter(|value| !value.trim().is_empty()) {
        upsert_session_env_var(&mut merged, SESSION_ENV_COLORTERM, colorterm.to_string());
    }
    upsert_session_env_var(&mut merged, SESSION_ENV_CHARSET, charset.to_string());

    for var in env {
        let name = var.name.trim();
        if name.is_empty() {
            continue;
        }
        upsert_session_env_var(&mut merged, name, var.value);
    }

    merged
}

fn upsert_session_env_var(env: &mut Vec<SessionEnvVar>, name: &str, value: String) {
    if let Some(var) = env.iter_mut().find(|var| var.name == name) {
        var.value = value;
    } else {
        env.push(SessionEnvVar {
            name: name.to_string(),
            value,
        });
    }
}

fn termua_db_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = tests::TERMUA_DB_PATH_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return path;
    }

    crate::settings::settings_dir_path().join("termua.db")
}

fn initialized_schema_paths() -> &'static Mutex<HashSet<PathBuf>> {
    static INITIALIZED_SCHEMA_PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    INITIALIZED_SCHEMA_PATHS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn schema_ready_for_path(path: &std::path::Path) -> bool {
    initialized_schema_paths()
        .lock()
        .ok()
        .is_some_and(|paths| paths.contains(path))
}

fn mark_schema_ready_for_path(path: &std::path::Path) {
    if let Ok(mut paths) = initialized_schema_paths().lock() {
        paths.insert(path.to_path_buf());
    }
}

fn open() -> anyhow::Result<Connection> {
    let path = termua_db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create termua db dir {parent:?}"))?;
        // The sessions DB can contain sensitive information; keep the directory private on Unix.
        let _ = chmod_private_dir(parent);
    }

    let conn = Connection::open(&path).with_context(|| format!("open termua db {path:?}"))?;
    if !schema_ready_for_path(&path) {
        init_schema(&conn)?;
        mark_schema_ready_for_path(&path);
    }
    ensure_private_sqlite_files(&path)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          protocol TEXT NOT NULL,
          group_path TEXT NOT NULL,
          label TEXT NOT NULL,
          backend TEXT NOT NULL,
          session_env TEXT,
          shell_program TEXT,
          ssh_host TEXT,
          ssh_port INTEGER,
          ssh_auth_type TEXT,
          ssh_user TEXT,
          ssh_credential_username TEXT,
          ssh_tcp_nodelay INTEGER NOT NULL DEFAULT 0,
          ssh_tcp_keepalive INTEGER NOT NULL DEFAULT 0,
          ssh_proxy_mode TEXT,
          ssh_proxy_command TEXT,
          ssh_proxy_workdir TEXT,
          ssh_proxy_env TEXT,
          ssh_proxy_jump TEXT,
          serial_port TEXT,
          serial_baud INTEGER,
          serial_data_bits INTEGER,
          serial_parity TEXT,
          serial_stop_bits INTEGER,
          serial_flow_control TEXT,
          created_at INTEGER NOT NULL DEFAULT (unixepoch()),
          updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );

        CREATE INDEX IF NOT EXISTS sessions_group_path_idx ON sessions(group_path);
        CREATE INDEX IF NOT EXISTS sessions_label_idx ON sessions(label);
        "#,
    )
    .context("create sessions schema")?;

    // Lightweight migration for existing DBs.
    ensure_sessions_column(conn, "ssh_proxy_mode", "TEXT")?;
    ensure_sessions_column(conn, "ssh_proxy_command", "TEXT")?;
    ensure_sessions_column(conn, "ssh_proxy_workdir", "TEXT")?;
    ensure_sessions_column(conn, "ssh_proxy_env", "TEXT")?;
    ensure_sessions_column(conn, "ssh_proxy_jump", "TEXT")?;
    ensure_sessions_column(conn, "ssh_tcp_nodelay", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_sessions_column(conn, "ssh_tcp_keepalive", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_sessions_column(conn, "session_env", "TEXT")?;
    ensure_sessions_column(conn, "serial_port", "TEXT")?;
    ensure_sessions_column(conn, "serial_baud", "INTEGER")?;
    ensure_sessions_column(conn, "serial_data_bits", "INTEGER")?;
    ensure_sessions_column(conn, "serial_parity", "TEXT")?;
    ensure_sessions_column(conn, "serial_stop_bits", "INTEGER")?;
    ensure_sessions_column(conn, "serial_flow_control", "TEXT")?;
    migrate_legacy_session_terminal_fields(conn)?;
    Ok(())
}

fn ensure_sessions_column(conn: &Connection, name: &str, ty: &str) -> anyhow::Result<()> {
    if sessions_has_column(conn, name)? {
        return Ok(());
    }

    conn.execute(&format!("ALTER TABLE sessions ADD COLUMN {name} {ty}"), [])
        .with_context(|| format!("add sessions column {name}"))?;
    Ok(())
}

fn sessions_has_column(conn: &Connection, name: &str) -> anyhow::Result<bool> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(sessions)")
        .context("prepare sessions table_info")?;
    let mut rows = stmt.query([]).context("query sessions table_info")?;
    while let Some(row) = rows.next().context("read sessions table_info row")? {
        let col_name: String = row.get(1)?;
        if col_name == name {
            return Ok(true);
        }
    }
    Ok(false)
}

fn migrate_legacy_session_terminal_fields(conn: &Connection) -> anyhow::Result<()> {
    let has_term = sessions_has_column(conn, "term")?;
    let has_charset = sessions_has_column(conn, "charset")?;
    let has_colorterm = sessions_has_column(conn, "colorterm")?;
    if !has_term || !has_charset || !has_colorterm {
        return Ok(());
    }

    let mut stmt = conn
        .prepare("SELECT id, term, charset, colorterm, session_env FROM sessions")
        .context("prepare migrate legacy terminal fields")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .context("query migrate legacy terminal fields")?;

    for row in rows {
        let (id, term, charset, colorterm, session_env_raw) = row?;
        let env = session_env_raw
            .and_then(|raw| serde_json_lenient::from_str::<Vec<SessionEnvVar>>(&raw).ok())
            .unwrap_or_default();
        let merged = merge_terminal_fields_into_env(&term, colorterm.as_deref(), &charset, env);
        let merged_json = serialize_session_env(&merged)?;
        conn.execute(
            "UPDATE sessions SET session_env = ?2 WHERE id = ?1",
            params![id, merged_json],
        )
        .with_context(|| format!("migrate legacy terminal fields for session {id}"))?;
    }

    Ok(())
}

fn backend_to_str(backend: TerminalBackend) -> &'static str {
    match backend {
        TerminalBackend::Alacritty => "alacritty",
        TerminalBackend::Wezterm => "wezterm",
    }
}

fn backend_from_str(s: &str) -> anyhow::Result<TerminalBackend> {
    Ok(match s {
        "alacritty" => TerminalBackend::Alacritty,
        "wezterm" => TerminalBackend::Wezterm,
        other => anyhow::bail!("unknown backend {other:?}"),
    })
}

fn protocol_to_str(protocol: &SessionType) -> &'static str {
    match protocol {
        SessionType::Local => "local",
        SessionType::Ssh => "ssh",
        SessionType::Serial => "serial",
    }
}

fn protocol_from_str(s: &str) -> anyhow::Result<SessionType> {
    Ok(match s {
        "local" => SessionType::Local,
        "ssh" => SessionType::Ssh,
        "serial" => SessionType::Serial,
        other => anyhow::bail!("unknown protocol {other:?}"),
    })
}

fn serial_parity_to_str(parity: SerialParity) -> &'static str {
    match parity {
        SerialParity::None => "none",
        SerialParity::Even => "even",
        SerialParity::Odd => "odd",
    }
}

fn serial_parity_from_str(s: &str) -> anyhow::Result<SerialParity> {
    Ok(match s {
        "none" => SerialParity::None,
        "even" => SerialParity::Even,
        "odd" => SerialParity::Odd,
        other => anyhow::bail!("unknown serial_parity {other:?}"),
    })
}

fn serial_flow_control_to_str(flow: SerialFlowControl) -> &'static str {
    match flow {
        SerialFlowControl::None => "none",
        SerialFlowControl::Software => "xonxoff",
        SerialFlowControl::Hardware => "rtscts",
    }
}

fn serial_flow_control_from_str(s: &str) -> anyhow::Result<SerialFlowControl> {
    Ok(match s {
        "none" => SerialFlowControl::None,
        "xonxoff" => SerialFlowControl::Software,
        "rtscts" => SerialFlowControl::Hardware,
        other => anyhow::bail!("unknown serial_flow_control {other:?}"),
    })
}

fn serial_stop_bits_to_i64(bits: SerialStopBits) -> i64 {
    match bits {
        SerialStopBits::One => 1,
        SerialStopBits::Two => 2,
    }
}

fn serial_stop_bits_from_i64(bits: i64) -> anyhow::Result<SerialStopBits> {
    Ok(match bits {
        1 => SerialStopBits::One,
        2 => SerialStopBits::Two,
        other => anyhow::bail!("unknown serial_stop_bits {other:?}"),
    })
}

fn ssh_auth_to_str(auth: &SshAuthType) -> &'static str {
    match auth {
        SshAuthType::Password => "password",
        SshAuthType::Config => "config",
    }
}

fn ssh_auth_from_str(s: &str) -> anyhow::Result<SshAuthType> {
    Ok(match s {
        "password" => SshAuthType::Password,
        "config" => SshAuthType::Config,
        other => anyhow::bail!("unknown ssh_auth_type {other:?}"),
    })
}

fn ssh_proxy_mode_to_str(mode: &SshProxyMode) -> &'static str {
    match mode {
        SshProxyMode::Inherit => "inherit",
        SshProxyMode::Disabled => "disabled",
        SshProxyMode::Command => "command",
        SshProxyMode::JumpServer => "jumpserver",
    }
}

fn ssh_proxy_mode_from_str(s: &str) -> anyhow::Result<SshProxyMode> {
    Ok(match s {
        "inherit" => SshProxyMode::Inherit,
        "disabled" => SshProxyMode::Disabled,
        "command" => SshProxyMode::Command,
        "jumpserver" => SshProxyMode::JumpServer,
        other => anyhow::bail!("unknown ssh_proxy_mode {other:?}"),
    })
}

struct SessionWrite<'a> {
    protocol: SessionType,
    group_path: &'a str,
    label: &'a str,
    backend: TerminalBackend,
    env: Vec<SessionEnvVar>,
    shell_program: Option<&'a str>,
    ssh_host: Option<&'a str>,
    ssh_port: Option<u16>,
    ssh_auth_type: Option<SshAuthType>,
    ssh_user: Option<&'a str>,
    ssh_tcp_nodelay: bool,
    ssh_tcp_keepalive: bool,
    ssh_proxy_mode: Option<SshProxyMode>,
    ssh_proxy_command: Option<&'a str>,
    ssh_proxy_workdir: Option<&'a str>,
    ssh_proxy_env: Vec<SshProxyEnvVar>,
    ssh_proxy_jump: Vec<SshJumpHop>,
    serial_port: Option<&'a str>,
    serial_baud: Option<u32>,
    serial_data_bits: Option<u8>,
    serial_parity: Option<SerialParity>,
    serial_stop_bits: Option<SerialStopBits>,
    serial_flow_control: Option<SerialFlowControl>,
    ssh_password: Option<&'a str>,
}

impl<'a> SessionWrite<'a> {
    fn local(
        group_path: &'a str,
        label: &'a str,
        backend: TerminalBackend,
        shell_program: &'a str,
        term: &'a str,
        colorterm: Option<&'a str>,
        charset: &'a str,
        env: Vec<SessionEnvVar>,
    ) -> Self {
        Self {
            protocol: SessionType::Local,
            group_path,
            label,
            backend,
            env: merge_terminal_fields_into_env(term, colorterm, charset, env),
            shell_program: Some(shell_program),
            ssh_host: None,
            ssh_port: None,
            ssh_auth_type: None,
            ssh_user: None,
            ssh_tcp_nodelay: false,
            ssh_tcp_keepalive: false,
            ssh_proxy_mode: None,
            ssh_proxy_command: None,
            ssh_proxy_workdir: None,
            ssh_proxy_env: Vec::new(),
            ssh_proxy_jump: Vec::new(),
            serial_port: None,
            serial_baud: None,
            serial_data_bits: None,
            serial_parity: None,
            serial_stop_bits: None,
            serial_flow_control: None,
            ssh_password: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ssh_password(
        group_path: &'a str,
        label: &'a str,
        backend: TerminalBackend,
        host: &'a str,
        port: u16,
        user: &'a str,
        password: &'a str,
        term: &'a str,
        colorterm: Option<&'a str>,
        charset: &'a str,
        env: Vec<SessionEnvVar>,
        ssh_tcp_nodelay: bool,
        ssh_tcp_keepalive: bool,
        proxy_mode: SshProxyMode,
        proxy_command: Option<&'a str>,
        proxy_workdir: Option<&'a str>,
        proxy_env: Vec<SshProxyEnvVar>,
        proxy_jump: Vec<SshJumpHop>,
    ) -> Self {
        Self {
            protocol: SessionType::Ssh,
            group_path,
            label,
            backend,
            env: merge_terminal_fields_into_env(term, colorterm, charset, env),
            shell_program: None,
            ssh_host: Some(host),
            ssh_port: Some(port),
            ssh_auth_type: Some(SshAuthType::Password),
            ssh_user: Some(user),
            ssh_tcp_nodelay,
            ssh_tcp_keepalive,
            ssh_proxy_mode: Some(proxy_mode),
            ssh_proxy_command: proxy_command,
            ssh_proxy_workdir: proxy_workdir,
            ssh_proxy_env: proxy_env,
            ssh_proxy_jump: proxy_jump,
            serial_port: None,
            serial_baud: None,
            serial_data_bits: None,
            serial_parity: None,
            serial_stop_bits: None,
            serial_flow_control: None,
            ssh_password: Some(password),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ssh_config(
        group_path: &'a str,
        label: &'a str,
        backend: TerminalBackend,
        host: &'a str,
        port: u16,
        term: &'a str,
        colorterm: Option<&'a str>,
        charset: &'a str,
        env: Vec<SessionEnvVar>,
        ssh_tcp_nodelay: bool,
        ssh_tcp_keepalive: bool,
        proxy_mode: SshProxyMode,
        proxy_command: Option<&'a str>,
        proxy_workdir: Option<&'a str>,
        proxy_env: Vec<SshProxyEnvVar>,
        proxy_jump: Vec<SshJumpHop>,
    ) -> Self {
        Self {
            protocol: SessionType::Ssh,
            group_path,
            label,
            backend,
            env: merge_terminal_fields_into_env(term, colorterm, charset, env),
            shell_program: None,
            ssh_host: Some(host),
            ssh_port: Some(port),
            ssh_auth_type: Some(SshAuthType::Config),
            ssh_user: None,
            ssh_tcp_nodelay,
            ssh_tcp_keepalive,
            ssh_proxy_mode: Some(proxy_mode),
            ssh_proxy_command: proxy_command,
            ssh_proxy_workdir: proxy_workdir,
            ssh_proxy_env: proxy_env,
            ssh_proxy_jump: proxy_jump,
            serial_port: None,
            serial_baud: None,
            serial_data_bits: None,
            serial_parity: None,
            serial_stop_bits: None,
            serial_flow_control: None,
            ssh_password: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn serial(
        group_path: &'a str,
        label: &'a str,
        backend: TerminalBackend,
        port: &'a str,
        baud: u32,
        data_bits: u8,
        parity: SerialParity,
        stop_bits: SerialStopBits,
        flow_control: SerialFlowControl,
        term: &'a str,
        charset: &'a str,
    ) -> Self {
        Self {
            protocol: SessionType::Serial,
            group_path,
            label,
            backend,
            env: merge_terminal_fields_into_env(term, None, charset, Vec::new()),
            shell_program: None,
            ssh_host: None,
            ssh_port: None,
            ssh_auth_type: None,
            ssh_user: None,
            ssh_tcp_nodelay: false,
            ssh_tcp_keepalive: false,
            ssh_proxy_mode: None,
            ssh_proxy_command: None,
            ssh_proxy_workdir: None,
            ssh_proxy_env: Vec::new(),
            ssh_proxy_jump: Vec::new(),
            serial_port: Some(port),
            serial_baud: Some(baud),
            serial_data_bits: Some(data_bits),
            serial_parity: Some(parity),
            serial_stop_bits: Some(stop_bits),
            serial_flow_control: Some(flow_control),
            ssh_password: None,
        }
    }
}

fn serialize_ssh_proxy_env(proxy_env: &[SshProxyEnvVar]) -> anyhow::Result<Option<String>> {
    if proxy_env.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::to_string(proxy_env).context("serialize ssh_proxy_env")?,
    ))
}

fn serialize_session_env(env: &[SessionEnvVar]) -> anyhow::Result<Option<String>> {
    if env.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::to_string(env).context("serialize session_env")?,
    ))
}

fn serialize_ssh_proxy_jump(
    proxy_mode: Option<SshProxyMode>,
    proxy_jump: &[SshJumpHop],
) -> anyhow::Result<Option<String>> {
    if proxy_mode != Some(SshProxyMode::JumpServer) || proxy_jump.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::to_string(proxy_jump).context("serialize ssh_proxy_jump")?,
    ))
}

fn insert_session_row(conn: &Connection, session: &SessionWrite<'_>) -> anyhow::Result<i64> {
    let proxy_env_json = serialize_ssh_proxy_env(&session.ssh_proxy_env)?;
    let proxy_jump_json =
        serialize_ssh_proxy_jump(session.ssh_proxy_mode, &session.ssh_proxy_jump)?;
    let session_env_json = serialize_session_env(&session.env)?;

    conn.execute(
        r#"
        INSERT INTO sessions (
          protocol, group_path, label,
          backend, session_env,
          shell_program,
          ssh_host, ssh_port, ssh_auth_type, ssh_user, ssh_credential_username,
          ssh_tcp_nodelay, ssh_tcp_keepalive,
          ssh_proxy_mode, ssh_proxy_command, ssh_proxy_workdir, ssh_proxy_env, ssh_proxy_jump,
          serial_port, serial_baud, serial_data_bits, serial_parity, serial_stop_bits, serial_flow_control
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
        "#,
        params![
            protocol_to_str(&session.protocol),
            session.group_path,
            session.label,
            backend_to_str(session.backend),
            session_env_json,
            session.shell_program,
            session.ssh_host,
            session.ssh_port.map(|value| value as i64),
            session.ssh_auth_type.as_ref().map(ssh_auth_to_str),
            session.ssh_user,
            if session.ssh_tcp_nodelay { 1 } else { 0 },
            if session.ssh_tcp_keepalive { 1 } else { 0 },
            session.ssh_proxy_mode.as_ref().map(ssh_proxy_mode_to_str),
            session.ssh_proxy_command,
            session.ssh_proxy_workdir,
            proxy_env_json,
            proxy_jump_json,
            session.serial_port,
            session.serial_baud.map(|value| value as i64),
            session.serial_data_bits.map(|value| value as i64),
            session.serial_parity.map(serial_parity_to_str),
            session.serial_stop_bits.map(serial_stop_bits_to_i64),
            session.serial_flow_control.map(serial_flow_control_to_str),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn update_session_row(
    conn: &Connection,
    id: i64,
    session: &SessionWrite<'_>,
) -> anyhow::Result<()> {
    let proxy_env_json = serialize_ssh_proxy_env(&session.ssh_proxy_env)?;
    let proxy_jump_json =
        serialize_ssh_proxy_jump(session.ssh_proxy_mode, &session.ssh_proxy_jump)?;
    let session_env_json = serialize_session_env(&session.env)?;

    conn.execute(
        r#"
        UPDATE sessions
        SET
          protocol = ?2,
          group_path = ?3,
          label = ?4,
          backend = ?5,
          session_env = ?6,
          shell_program = ?7,
          ssh_host = ?8,
          ssh_port = ?9,
          ssh_auth_type = ?10,
          ssh_user = ?11,
          ssh_credential_username = NULL,
          ssh_tcp_nodelay = ?12,
          ssh_tcp_keepalive = ?13,
          ssh_proxy_mode = ?14,
          ssh_proxy_command = ?15,
          ssh_proxy_workdir = ?16,
          ssh_proxy_env = ?17,
          ssh_proxy_jump = ?18,
          serial_port = ?19,
          serial_baud = ?20,
          serial_data_bits = ?21,
          serial_parity = ?22,
          serial_stop_bits = ?23,
          serial_flow_control = ?24,
          updated_at = unixepoch()
        WHERE id = ?1
        "#,
        params![
            id,
            protocol_to_str(&session.protocol),
            session.group_path,
            session.label,
            backend_to_str(session.backend),
            session_env_json,
            session.shell_program,
            session.ssh_host,
            session.ssh_port.map(|value| value as i64),
            session.ssh_auth_type.as_ref().map(ssh_auth_to_str),
            session.ssh_user,
            if session.ssh_tcp_nodelay { 1 } else { 0 },
            if session.ssh_tcp_keepalive { 1 } else { 0 },
            session.ssh_proxy_mode.as_ref().map(ssh_proxy_mode_to_str),
            session.ssh_proxy_command,
            session.ssh_proxy_workdir,
            proxy_env_json,
            proxy_jump_json,
            session.serial_port,
            session.serial_baud.map(|value| value as i64),
            session.serial_data_bits.map(|value| value as i64),
            session.serial_parity.map(serial_parity_to_str),
            session.serial_stop_bits.map(serial_stop_bits_to_i64),
            session.serial_flow_control.map(serial_flow_control_to_str),
        ],
    )?;
    Ok(())
}

fn delete_ssh_password_if_present(id: i64) {
    if let Err(err) = crate::keychain::delete_ssh_password(id) {
        log::warn!(
            "failed to delete ssh password from credential manager for session {id}: {err:#}"
        );
    }
}

fn store_ssh_password_if_present(id: i64, password: Option<&str>) {
    let Some(password) = password else {
        return;
    };

    if let Err(err) = crate::keychain::store_ssh_password(id, password) {
        log::warn!("failed to store ssh password in credential manager for session {id}: {err:#}");
    }
}

pub fn save_local_session(
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    shell_program: &str,
    term: &str,
    charset: &str,
) -> anyhow::Result<i64> {
    save_local_session_with_env(
        group_path,
        label,
        backend,
        shell_program,
        term,
        None,
        charset,
        Vec::new(),
    )
}

pub fn save_local_session_with_env(
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    shell_program: &str,
    term: &str,
    colorterm: Option<&str>,
    charset: &str,
    env: Vec<SessionEnvVar>,
) -> anyhow::Result<i64> {
    let conn = open()?;
    insert_session_row(
        &conn,
        &SessionWrite::local(
            group_path,
            label,
            backend,
            shell_program,
            term,
            colorterm,
            charset,
            env,
        ),
    )
    .context("insert local session")
}

pub fn save_ssh_session_password(
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    term: &str,
    charset: &str,
) -> anyhow::Result<i64> {
    save_ssh_session_password_with_proxy(
        group_path,
        label,
        backend,
        host,
        port,
        user,
        password,
        term,
        charset,
        true,
        false,
        SshProxyMode::Inherit,
        None,
        None,
        Vec::new(),
        Vec::new(),
    )
}

pub fn save_ssh_session_password_with_proxy(
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    term: &str,
    charset: &str,
    ssh_tcp_nodelay: bool,
    ssh_tcp_keepalive: bool,
    proxy_mode: SshProxyMode,
    proxy_command: Option<&str>,
    proxy_workdir: Option<&str>,
    proxy_env: Vec<SshProxyEnvVar>,
    proxy_jump: Vec<SshJumpHop>,
) -> anyhow::Result<i64> {
    save_ssh_session_password_with_proxy_and_env(
        group_path,
        label,
        backend,
        host,
        port,
        user,
        password,
        term,
        None,
        charset,
        ssh_tcp_nodelay,
        ssh_tcp_keepalive,
        proxy_mode,
        proxy_command,
        proxy_workdir,
        proxy_env,
        proxy_jump,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn save_ssh_session_password_with_proxy_and_env(
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    term: &str,
    colorterm: Option<&str>,
    charset: &str,
    ssh_tcp_nodelay: bool,
    ssh_tcp_keepalive: bool,
    proxy_mode: SshProxyMode,
    proxy_command: Option<&str>,
    proxy_workdir: Option<&str>,
    proxy_env: Vec<SshProxyEnvVar>,
    proxy_jump: Vec<SshJumpHop>,
    env: Vec<SessionEnvVar>,
) -> anyhow::Result<i64> {
    let conn = open()?;
    let session = SessionWrite::ssh_password(
        group_path,
        label,
        backend,
        host,
        port,
        user,
        password,
        term,
        colorterm,
        charset,
        env,
        ssh_tcp_nodelay,
        ssh_tcp_keepalive,
        proxy_mode,
        proxy_command,
        proxy_workdir,
        proxy_env,
        proxy_jump,
    );
    let id = insert_session_row(&conn, &session).context("insert ssh password session")?;
    store_ssh_password_if_present(id, session.ssh_password);
    Ok(id)
}

pub fn save_ssh_session_config(
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    term: &str,
    charset: &str,
) -> anyhow::Result<i64> {
    save_ssh_session_config_with_proxy(
        group_path,
        label,
        backend,
        host,
        port,
        term,
        charset,
        true,
        false,
        SshProxyMode::Inherit,
        None,
        None,
        Vec::new(),
        Vec::new(),
    )
}

pub fn save_ssh_session_config_with_proxy(
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    term: &str,
    charset: &str,
    ssh_tcp_nodelay: bool,
    ssh_tcp_keepalive: bool,
    proxy_mode: SshProxyMode,
    proxy_command: Option<&str>,
    proxy_workdir: Option<&str>,
    proxy_env: Vec<SshProxyEnvVar>,
    proxy_jump: Vec<SshJumpHop>,
) -> anyhow::Result<i64> {
    save_ssh_session_config_with_proxy_and_env(
        group_path,
        label,
        backend,
        host,
        port,
        term,
        None,
        charset,
        ssh_tcp_nodelay,
        ssh_tcp_keepalive,
        proxy_mode,
        proxy_command,
        proxy_workdir,
        proxy_env,
        proxy_jump,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn save_ssh_session_config_with_proxy_and_env(
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    term: &str,
    colorterm: Option<&str>,
    charset: &str,
    ssh_tcp_nodelay: bool,
    ssh_tcp_keepalive: bool,
    proxy_mode: SshProxyMode,
    proxy_command: Option<&str>,
    proxy_workdir: Option<&str>,
    proxy_env: Vec<SshProxyEnvVar>,
    proxy_jump: Vec<SshJumpHop>,
    env: Vec<SessionEnvVar>,
) -> anyhow::Result<i64> {
    let conn = open()?;
    insert_session_row(
        &conn,
        &SessionWrite::ssh_config(
            group_path,
            label,
            backend,
            host,
            port,
            term,
            colorterm,
            charset,
            env,
            ssh_tcp_nodelay,
            ssh_tcp_keepalive,
            proxy_mode,
            proxy_command,
            proxy_workdir,
            proxy_env,
            proxy_jump,
        ),
    )
    .context("insert ssh config session")
}

pub fn save_serial_session(
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    port: &str,
    baud: u32,
    data_bits: u8,
    parity: SerialParity,
    stop_bits: SerialStopBits,
    flow_control: SerialFlowControl,
    term: &str,
    charset: &str,
) -> anyhow::Result<i64> {
    let conn = open()?;
    insert_session_row(
        &conn,
        &SessionWrite::serial(
            group_path,
            label,
            backend,
            port,
            baud,
            data_bits,
            parity,
            stop_bits,
            flow_control,
            term,
            charset,
        ),
    )
    .context("insert serial session")
}

pub fn load_all_sessions() -> anyhow::Result<Vec<Session>> {
    let conn = open()?;
    let mut stmt = conn
        .prepare(
            r#"
	            SELECT
	              id, protocol, group_path, label,
	              backend, session_env,
              shell_program,
              ssh_host, ssh_port, ssh_auth_type, ssh_user, ssh_credential_username,
              ssh_tcp_nodelay, ssh_tcp_keepalive,
              ssh_proxy_mode, ssh_proxy_command, ssh_proxy_workdir, ssh_proxy_env, ssh_proxy_jump,
              serial_port, serial_baud, serial_data_bits, serial_parity, serial_stop_bits, serial_flow_control
	            FROM sessions
	            ORDER BY group_path ASC, label ASC, id ASC
	            "#,
        )
        .context("prepare load sessions")?;

    let rows = stmt
        .query_map([], |row| parse_session_row(row, false))
        .context("query sessions")?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn load_session(id: i64) -> anyhow::Result<Option<Session>> {
    let conn = open()?;
    conn.query_row(
        r#"
	    SELECT
	      id, protocol, group_path, label,
	      backend, session_env,
              shell_program,
              ssh_host, ssh_port, ssh_auth_type, ssh_user, ssh_credential_username,
              ssh_tcp_nodelay, ssh_tcp_keepalive,
              ssh_proxy_mode, ssh_proxy_command, ssh_proxy_workdir, ssh_proxy_env, ssh_proxy_jump,
              serial_port, serial_baud, serial_data_bits, serial_parity, serial_stop_bits, serial_flow_control
	    FROM sessions
	    WHERE id = ?1
        "#,
        params![id],
        |row| parse_session_row(row, true),
    )
    .optional()
    .context("load session")
}

fn parse_session_row(row: &rusqlite::Row<'_>, include_password: bool) -> rusqlite::Result<Session> {
    let id: i64 = row.get(0)?;
    let protocol = parse_protocol(row)?;
    let backend = parse_backend(row)?;

    let ssh_auth_type = parse_ssh_auth_type(row)?;
    let ssh_password = if include_password {
        load_ssh_password_if_needed(id, &ssh_auth_type)
    } else {
        None
    };

    Ok(Session {
        id,
        protocol,
        group_path: row.get(2)?,
        label: row.get(3)?,
        backend,
        env: parse_session_env(row)?,
        shell_program: row.get(6)?,
        ssh_host: row.get(7)?,
        ssh_port: row.get::<_, Option<i64>>(8)?.map(|v| v as u16),
        ssh_auth_type,
        ssh_user: row.get(10)?,
        ssh_credential_username: row.get(11)?,
        ssh_password,

        ssh_tcp_nodelay: row.get::<_, i64>(12)? != 0,
        ssh_tcp_keepalive: row.get::<_, i64>(13)? != 0,

        ssh_proxy_mode: parse_ssh_proxy_mode(row)?,
        ssh_proxy_command: row.get(15)?,
        ssh_proxy_workdir: row.get(16)?,
        ssh_proxy_env: parse_ssh_proxy_env(row)?,
        ssh_proxy_jump: parse_ssh_proxy_jump(row)?,

        serial_port: row.get(19)?,
        serial_baud: row.get::<_, Option<i64>>(20)?.map(|v| v as u32),
        serial_data_bits: row.get::<_, Option<i64>>(21)?.map(|v| v as u8),
        serial_parity: parse_serial_parity(row)?,
        serial_stop_bits: parse_serial_stop_bits(row)?,
        serial_flow_control: parse_serial_flow_control(row)?,
    })
}

fn parse_protocol(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionType> {
    let protocol_s: String = row.get(1)?;
    protocol_from_str(&protocol_s)
        .map_err(|e| from_sql_text_parse_error(1, ParseError(e.to_string())))
}

fn parse_backend(row: &rusqlite::Row<'_>) -> rusqlite::Result<TerminalBackend> {
    let backend_s: String = row.get(4)?;
    backend_from_str(&backend_s)
        .map_err(|e| from_sql_text_parse_error(4, ParseError(e.to_string())))
}

fn parse_ssh_auth_type(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<SshAuthType>> {
    let ssh_auth_s: Option<String> = row.get(9)?;
    ssh_auth_s
        .map(|s| {
            ssh_auth_from_str(&s)
                .map_err(|e| from_sql_text_parse_error(9, ParseError(e.to_string())))
        })
        .transpose()
}

fn parse_session_env(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Vec<SessionEnvVar>>> {
    let session_env_s: Option<String> = row.get(5)?;
    Ok(session_env_s.and_then(|raw| serde_json_lenient::from_str::<Vec<SessionEnvVar>>(&raw).ok()))
}

fn parse_ssh_proxy_mode(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<SshProxyMode>> {
    let ssh_proxy_mode_s: Option<String> = row.get(14)?;
    ssh_proxy_mode_s
        .map(|s| {
            ssh_proxy_mode_from_str(&s)
                .map_err(|e| from_sql_text_parse_error(14, ParseError(e.to_string())))
        })
        .transpose()
}

fn parse_ssh_proxy_env(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Vec<SshProxyEnvVar>>> {
    let ssh_proxy_env_s: Option<String> = row.get(17)?;
    Ok(ssh_proxy_env_s
        .and_then(|raw| serde_json_lenient::from_str::<Vec<SshProxyEnvVar>>(&raw).ok()))
}

fn parse_ssh_proxy_jump(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Vec<SshJumpHop>>> {
    let ssh_proxy_jump_s: Option<String> = row.get(18)?;
    Ok(ssh_proxy_jump_s.and_then(|raw| serde_json_lenient::from_str::<Vec<SshJumpHop>>(&raw).ok()))
}

fn parse_serial_parity(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<SerialParity>> {
    let serial_parity_s: Option<String> = row.get(22)?;
    serial_parity_s
        .map(|s| {
            serial_parity_from_str(&s)
                .map_err(|e| from_sql_text_parse_error(22, ParseError(e.to_string())))
        })
        .transpose()
}

fn parse_serial_stop_bits(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<SerialStopBits>> {
    row.get::<_, Option<i64>>(23)?
        .map(|v| {
            serial_stop_bits_from_i64(v)
                .map_err(|e| from_sql_int_parse_error(23, ParseError(e.to_string())))
        })
        .transpose()
}

fn parse_serial_flow_control(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Option<SerialFlowControl>> {
    let serial_flow_control_s: Option<String> = row.get(24)?;
    serial_flow_control_s
        .map(|s| {
            serial_flow_control_from_str(&s)
                .map_err(|e| from_sql_text_parse_error(24, ParseError(e.to_string())))
        })
        .transpose()
}

fn load_ssh_password_if_needed(id: i64, ssh_auth_type: &Option<SshAuthType>) -> Option<String> {
    if *ssh_auth_type != Some(SshAuthType::Password) {
        return None;
    }

    match crate::keychain::load_ssh_password(id) {
        Ok(pw) => pw,
        Err(err) => {
            log::warn!(
                "failed to load ssh password from credential manager for session {id}: {err:#}"
            );
            None
        }
    }
}

fn from_sql_text_parse_error(col: usize, err: ParseError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, Box::new(err))
}

fn from_sql_int_parse_error(col: usize, err: ParseError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Integer, Box::new(err))
}

pub fn delete_session(id: i64) -> anyhow::Result<()> {
    if let Err(err) = crate::keychain::delete_ssh_password(id) {
        log::warn!(
            "failed to delete ssh password from credential manager for session {id}: {err:#}"
        );
    }
    let conn = open()?;
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])
        .context("delete session")?;
    Ok(())
}

pub fn update_local_session(
    id: i64,
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    shell_program: &str,
    term: &str,
    charset: &str,
) -> anyhow::Result<()> {
    update_local_session_with_env(
        id,
        group_path,
        label,
        backend,
        shell_program,
        term,
        None,
        charset,
        Vec::new(),
    )
}

pub fn update_local_session_with_env(
    id: i64,
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    shell_program: &str,
    term: &str,
    colorterm: Option<&str>,
    charset: &str,
    env: Vec<SessionEnvVar>,
) -> anyhow::Result<()> {
    delete_ssh_password_if_present(id);
    let conn = open()?;
    update_session_row(
        &conn,
        id,
        &SessionWrite::local(
            group_path,
            label,
            backend,
            shell_program,
            term,
            colorterm,
            charset,
            env,
        ),
    )
    .context("update local session")
}

pub fn update_ssh_session_password(
    id: i64,
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    term: &str,
    charset: &str,
) -> anyhow::Result<()> {
    update_ssh_session_password_with_proxy(
        id,
        group_path,
        label,
        backend,
        host,
        port,
        user,
        password,
        term,
        charset,
        true,
        false,
        SshProxyMode::Inherit,
        None,
        None,
        Vec::new(),
        Vec::new(),
    )
}

pub fn update_ssh_session_password_with_proxy(
    id: i64,
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    term: &str,
    charset: &str,
    ssh_tcp_nodelay: bool,
    ssh_tcp_keepalive: bool,
    proxy_mode: SshProxyMode,
    proxy_command: Option<&str>,
    proxy_workdir: Option<&str>,
    proxy_env: Vec<SshProxyEnvVar>,
    proxy_jump: Vec<SshJumpHop>,
) -> anyhow::Result<()> {
    update_ssh_session_password_with_proxy_and_env(
        id,
        group_path,
        label,
        backend,
        host,
        port,
        user,
        password,
        term,
        None,
        charset,
        ssh_tcp_nodelay,
        ssh_tcp_keepalive,
        proxy_mode,
        proxy_command,
        proxy_workdir,
        proxy_env,
        proxy_jump,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn update_ssh_session_password_with_proxy_and_env(
    id: i64,
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    term: &str,
    colorterm: Option<&str>,
    charset: &str,
    ssh_tcp_nodelay: bool,
    ssh_tcp_keepalive: bool,
    proxy_mode: SshProxyMode,
    proxy_command: Option<&str>,
    proxy_workdir: Option<&str>,
    proxy_env: Vec<SshProxyEnvVar>,
    proxy_jump: Vec<SshJumpHop>,
    env: Vec<SessionEnvVar>,
) -> anyhow::Result<()> {
    let conn = open()?;
    let session = SessionWrite::ssh_password(
        group_path,
        label,
        backend,
        host,
        port,
        user,
        password,
        term,
        colorterm,
        charset,
        env,
        ssh_tcp_nodelay,
        ssh_tcp_keepalive,
        proxy_mode,
        proxy_command,
        proxy_workdir,
        proxy_env,
        proxy_jump,
    );
    update_session_row(&conn, id, &session).context("update ssh password session")?;
    store_ssh_password_if_present(id, session.ssh_password);
    Ok(())
}

pub fn update_ssh_session_config(
    id: i64,
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    term: &str,
    charset: &str,
) -> anyhow::Result<()> {
    update_ssh_session_config_with_proxy(
        id,
        group_path,
        label,
        backend,
        host,
        port,
        term,
        charset,
        true,
        false,
        SshProxyMode::Inherit,
        None,
        None,
        Vec::new(),
        Vec::new(),
    )
}

pub fn update_ssh_session_config_with_proxy(
    id: i64,
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    term: &str,
    charset: &str,
    ssh_tcp_nodelay: bool,
    ssh_tcp_keepalive: bool,
    proxy_mode: SshProxyMode,
    proxy_command: Option<&str>,
    proxy_workdir: Option<&str>,
    proxy_env: Vec<SshProxyEnvVar>,
    proxy_jump: Vec<SshJumpHop>,
) -> anyhow::Result<()> {
    update_ssh_session_config_with_proxy_and_env(
        id,
        group_path,
        label,
        backend,
        host,
        port,
        term,
        None,
        charset,
        ssh_tcp_nodelay,
        ssh_tcp_keepalive,
        proxy_mode,
        proxy_command,
        proxy_workdir,
        proxy_env,
        proxy_jump,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn update_ssh_session_config_with_proxy_and_env(
    id: i64,
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    host: &str,
    port: u16,
    term: &str,
    colorterm: Option<&str>,
    charset: &str,
    ssh_tcp_nodelay: bool,
    ssh_tcp_keepalive: bool,
    proxy_mode: SshProxyMode,
    proxy_command: Option<&str>,
    proxy_workdir: Option<&str>,
    proxy_env: Vec<SshProxyEnvVar>,
    proxy_jump: Vec<SshJumpHop>,
    env: Vec<SessionEnvVar>,
) -> anyhow::Result<()> {
    delete_ssh_password_if_present(id);
    let conn = open()?;
    update_session_row(
        &conn,
        id,
        &SessionWrite::ssh_config(
            group_path,
            label,
            backend,
            host,
            port,
            term,
            colorterm,
            charset,
            env,
            ssh_tcp_nodelay,
            ssh_tcp_keepalive,
            proxy_mode,
            proxy_command,
            proxy_workdir,
            proxy_env,
            proxy_jump,
        ),
    )
    .context("update ssh config session")
}

pub fn update_serial_session(
    id: i64,
    group_path: &str,
    label: &str,
    backend: TerminalBackend,
    port: &str,
    baud: u32,
    data_bits: u8,
    parity: SerialParity,
    stop_bits: SerialStopBits,
    flow_control: SerialFlowControl,
    term: &str,
    charset: &str,
) -> anyhow::Result<()> {
    delete_ssh_password_if_present(id);

    let conn = open()?;
    update_session_row(
        &conn,
        id,
        &SessionWrite::serial(
            group_path,
            label,
            backend,
            port,
            baud,
            data_bits,
            parity,
            stop_bits,
            flow_control,
            term,
            charset,
        ),
    )
    .context("update serial session")
}

pub fn update_serial_session_port(id: i64, port: &str) -> anyhow::Result<()> {
    let conn = open()?;
    conn.execute(
        r#"
        UPDATE sessions
        SET
          serial_port = ?2,
          updated_at = unixepoch()
        WHERE id = ?1
        "#,
        params![id, port],
    )
    .context("update serial session port")?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    thread_local! {
        pub static TERMUA_DB_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
            const { std::cell::RefCell::new(None) };
    }

    pub struct TermuaDbPathOverrideGuard {
        prev: Option<PathBuf>,
    }

    impl Drop for TermuaDbPathOverrideGuard {
        fn drop(&mut self) {
            let prev = self.prev.take();
            TERMUA_DB_PATH_OVERRIDE.with(|slot| *slot.borrow_mut() = prev);
        }
    }

    pub fn override_termua_db_path(path: PathBuf) -> TermuaDbPathOverrideGuard {
        let prev = TERMUA_DB_PATH_OVERRIDE.with(|slot| slot.borrow_mut().replace(path));
        TermuaDbPathOverrideGuard { prev }
    }

    fn unique_test_db_path(name: &str) -> PathBuf {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        let nonce = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp_dir = std::env::temp_dir().join(format!(
            "termua-sessions-test-{name}-{}-{timestamp}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        tmp_dir.join("termua").join("termua.db")
    }

    #[test]
    fn protocol_from_str_accepts_serial() {
        assert_eq!(protocol_from_str("serial").unwrap(), SessionType::Serial);
    }

    #[test]
    fn sessions_can_be_saved_and_loaded_from_sqlite() {
        let db_path = unique_test_db_path("basic");
        let _guard = override_termua_db_path(db_path);

        let local_id = save_local_session(
            "local>dev",
            "bash",
            TerminalBackend::Wezterm,
            "bash",
            "xterm-256color",
            "UTF-8",
        )
        .unwrap();
        assert!(local_id > 0);

        let ssh_id = save_ssh_session_config(
            "ssh",
            "prod",
            TerminalBackend::Alacritty,
            "example.com",
            22,
            "xterm-256color",
            "UTF-8",
        )
        .unwrap();
        assert!(ssh_id > 0);

        let all = load_all_sessions().unwrap();
        assert_eq!(all.len(), 2);
        assert!(
            all.iter()
                .any(|s| s.id == local_id && s.protocol == SessionType::Local)
        );
        assert!(
            all.iter()
                .any(|s| s.id == ssh_id && s.protocol == SessionType::Ssh)
        );

        let ssh = load_session(ssh_id).unwrap().unwrap();
        assert_eq!(ssh.label, "prod");
        assert_eq!(ssh.ssh_host.as_deref(), Some("example.com"));
    }

    #[test]
    fn sessions_schema_includes_ssh_tcp_socket_options() {
        let db_path = unique_test_db_path("tcp-options");
        let _guard = override_termua_db_path(db_path);

        let conn = open().unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(sessions)").unwrap();
        let mut rows = stmt.query([]).unwrap();

        let mut names = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let name: String = row.get(1).unwrap();
            names.push(name);
        }

        assert!(names.iter().any(|n| n == "ssh_tcp_nodelay"));
        assert!(names.iter().any(|n| n == "ssh_tcp_keepalive"));
    }

    #[test]
    fn sessions_schema_stores_terminal_fields_in_session_env_only() {
        let db_path = unique_test_db_path("session-env-schema");
        let _guard = override_termua_db_path(db_path);

        let conn = open().unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(sessions)").unwrap();
        let mut rows = stmt.query([]).unwrap();

        let mut names = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let name: String = row.get(1).unwrap();
            names.push(name);
        }

        assert!(names.iter().any(|n| n == "session_env"));
        assert!(!names.iter().any(|n| n == "term"));
        assert!(!names.iter().any(|n| n == "charset"));
        assert!(!names.iter().any(|n| n == "colorterm"));
    }

    #[cfg(unix)]
    #[test]
    fn termua_db_is_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let db_path = unique_test_db_path("db-perms");
        let _guard = override_termua_db_path(db_path.clone());

        let _id = save_local_session(
            "local",
            "bash",
            TerminalBackend::Wezterm,
            "bash",
            "xterm-256color",
            "UTF-8",
        )
        .unwrap();

        let mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "termua.db should be 0600 on Unix");

        if let Some(tmp_dir) = db_path.parent().and_then(|p| p.parent()) {
            let _ = std::fs::remove_dir_all(tmp_dir);
        }
    }

    #[test]
    fn ssh_password_sessions_do_not_store_password_in_plaintext() {
        let db_path = unique_test_db_path("password");
        let _guard = override_termua_db_path(db_path);

        let id = save_ssh_session_password(
            "ssh",
            "prod",
            TerminalBackend::Wezterm,
            "example.com",
            22,
            "root",
            "pw123",
            "xterm-256color",
            "UTF-8",
        )
        .unwrap();

        let conn = Connection::open(termua_db_path()).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(sessions)").unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let col_name: String = row.get(1).unwrap();
            assert_ne!(
                col_name, "ssh_password",
                "ssh_password column must not exist in termua.db"
            );
        }

        let loaded = load_session(id).unwrap().unwrap();
        let _ = loaded;
    }

    #[test]
    fn sessions_can_be_deleted_from_sqlite() {
        let db_path = unique_test_db_path("delete");
        let _guard = override_termua_db_path(db_path);

        let id = save_local_session(
            "local",
            "bash",
            TerminalBackend::Wezterm,
            "bash",
            "xterm-256color",
            "UTF-8",
        )
        .unwrap();
        assert!(load_session(id).unwrap().is_some());

        delete_session(id).unwrap();
        assert!(load_session(id).unwrap().is_none());
    }

    #[test]
    fn sessions_can_be_updated_in_sqlite() {
        let db_path = unique_test_db_path("update");
        let _guard = override_termua_db_path(db_path);

        let local_id = save_local_session(
            "local",
            "bash",
            TerminalBackend::Wezterm,
            "bash",
            "xterm-256color",
            "UTF-8",
        )
        .unwrap();

        update_local_session(
            local_id,
            "local>dev",
            "zsh",
            TerminalBackend::Alacritty,
            "zsh",
            "screen-256color",
            "ASCII",
        )
        .unwrap();

        let local = load_session(local_id).unwrap().unwrap();
        assert_eq!(local.group_path, "local>dev");
        assert_eq!(local.label, "zsh");
        assert_eq!(local.backend, TerminalBackend::Alacritty);
        assert_eq!(local.shell_program.as_deref(), Some("zsh"));
        assert_eq!(local.term(), "screen-256color");
        assert_eq!(local.charset(), "ASCII");

        let ssh_id = save_ssh_session_password(
            "ssh",
            "prod",
            TerminalBackend::Wezterm,
            "example.com",
            22,
            "root",
            "pw123",
            "xterm-256color",
            "UTF-8",
        )
        .unwrap();

        update_ssh_session_password(
            ssh_id,
            "ssh>staging",
            "staging",
            TerminalBackend::Wezterm,
            "api.example.com",
            2222,
            "alice",
            "newpw",
            "tmux-256color",
            "UTF-8",
        )
        .unwrap();

        let ssh = load_session(ssh_id).unwrap().unwrap();
        assert_eq!(ssh.group_path, "ssh>staging");
        assert_eq!(ssh.label, "staging");
        assert_eq!(ssh.ssh_host.as_deref(), Some("api.example.com"));
        assert_eq!(ssh.ssh_port, Some(2222));
        assert_eq!(ssh.ssh_user.as_deref(), Some("alice"));
        assert_eq!(ssh.term(), "tmux-256color");
    }

    #[test]
    fn ssh_proxy_settings_roundtrip_through_sqlite() {
        let db_path = unique_test_db_path("proxy");
        let _guard = override_termua_db_path(db_path);

        let env = vec![SshProxyEnvVar {
            name: "HTTP_PROXY".to_string(),
            value: "http://127.0.0.1:3128".to_string(),
        }];

        let id = save_ssh_session_password_with_proxy(
            "ssh",
            "proxy",
            TerminalBackend::Wezterm,
            "example.com",
            22,
            "root",
            "pw123",
            "xterm-256color",
            "UTF-8",
            false,
            false,
            SshProxyMode::Command,
            Some("nc -x 127.0.0.1:1080 %h %p"),
            Some("/tmp"),
            env.clone(),
            Vec::new(),
        )
        .unwrap();

        let ssh = load_session(id).unwrap().unwrap();
        assert_eq!(ssh.ssh_proxy_mode, Some(SshProxyMode::Command));
        assert_eq!(
            ssh.ssh_proxy_command.as_deref(),
            Some("nc -x 127.0.0.1:1080 %h %p")
        );
        assert_eq!(ssh.ssh_proxy_workdir.as_deref(), Some("/tmp"));
        assert_eq!(ssh.ssh_proxy_env.as_deref(), Some(env.as_slice()));
    }

    #[test]
    fn local_env_settings_roundtrip_through_sqlite() {
        let db_path = unique_test_db_path("local-env");
        let _guard = override_termua_db_path(db_path);

        let env = vec![
            SessionEnvVar {
                name: "COLORTERM".to_string(),
                value: "24bit".to_string(),
            },
            SessionEnvVar {
                name: "FOO".to_string(),
                value: "bar".to_string(),
            },
        ];

        let id = save_local_session_with_env(
            "local",
            "fish",
            TerminalBackend::Wezterm,
            "fish",
            "xterm-256color",
            Some("truecolor"),
            "UTF-8",
            env,
        )
        .unwrap();

        let local = load_session(id).unwrap().unwrap();
        assert_eq!(local.term(), "xterm-256color");
        assert_eq!(local.colorterm(), Some("24bit"));
        assert_eq!(local.charset(), "UTF-8");
        let env = local.env.as_deref().unwrap();
        assert!(
            env.iter()
                .any(|var| var.name == "TERM" && var.value == "xterm-256color")
        );
        assert!(
            env.iter()
                .any(|var| var.name == "COLORTERM" && var.value == "24bit")
        );
        assert!(
            env.iter()
                .any(|var| var.name == "CHARSET" && var.value == "UTF-8")
        );
        assert!(
            env.iter()
                .any(|var| var.name == "FOO" && var.value == "bar")
        );
    }

    #[test]
    fn ssh_remote_env_settings_roundtrip_through_sqlite() {
        let db_path = unique_test_db_path("ssh-env");
        let _guard = override_termua_db_path(db_path);

        let env = vec![SessionEnvVar {
            name: "LANG".to_string(),
            value: "C.UTF-8".to_string(),
        }];

        let id = save_ssh_session_config_with_proxy_and_env(
            "ssh",
            "prod",
            TerminalBackend::Wezterm,
            "example.com",
            22,
            "xterm-256color",
            Some("truecolor"),
            "UTF-8",
            true,
            false,
            SshProxyMode::Disabled,
            None,
            None,
            Vec::new(),
            Vec::new(),
            env,
        )
        .unwrap();

        let ssh = load_session(id).unwrap().unwrap();
        assert_eq!(ssh.colorterm(), Some("truecolor"));
        let session_env = ssh.env.as_deref().unwrap();
        assert!(
            session_env
                .iter()
                .any(|var| var.name == "TERM" && var.value == "xterm-256color")
        );
        assert!(
            session_env
                .iter()
                .any(|var| var.name == "COLORTERM" && var.value == "truecolor")
        );
        assert!(
            session_env
                .iter()
                .any(|var| var.name == "CHARSET" && var.value == "UTF-8")
        );
        assert!(
            session_env
                .iter()
                .any(|var| var.name == "LANG" && var.value == "C.UTF-8")
        );
    }

    #[test]
    fn ssh_jumpserver_proxy_settings_roundtrip_through_sqlite() {
        let db_path = unique_test_db_path("jumpserver");
        let _guard = override_termua_db_path(db_path);

        let hops = vec![
            SshJumpHop {
                host: "jump1".to_string(),
                user: Some("a".to_string()),
                port: Some(22),
            },
            SshJumpHop {
                host: "jump2".to_string(),
                user: None,
                port: Some(2201),
            },
            SshJumpHop {
                host: "jump3".to_string(),
                user: Some("c".to_string()),
                port: None,
            },
        ];

        let id = save_ssh_session_password_with_proxy(
            "ssh",
            "jump",
            TerminalBackend::Wezterm,
            "example.com",
            22,
            "root",
            "pw123",
            "xterm-256color",
            "UTF-8",
            false,
            false,
            SshProxyMode::JumpServer,
            None,
            None,
            Vec::new(),
            hops.clone(),
        )
        .unwrap();

        let ssh = load_session(id).unwrap().unwrap();
        assert_eq!(ssh.ssh_proxy_mode, Some(SshProxyMode::JumpServer));
        assert_eq!(ssh.ssh_proxy_jump.as_deref(), Some(hops.as_slice()));
    }
}
