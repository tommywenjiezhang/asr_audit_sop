//! Shared team database: connection abstraction, schema, identity and connection checks.
//!
//! Production storage is a Microsoft Access (.accdb) file on a network share, reached through the
//! 64-bit "Microsoft Access Driver (*.mdb, *.accdb)" ODBC driver (`AccessConn`). Automated tests use an
//! isolated SQLite file with the same tables (`SqliteShared`), so the transactional apply logic in
//! `shared_apply` is exercised by both backends. SQL used here is deliberately plain (no COALESCE, no
//! LIMIT/TOP outside `Dialect::limit`, parameters only as `?`) so it runs on both.
//!
//! Safety rules:
//! - The app never creates or overwrites a database at the configured path implicitly. `create_database`
//!   refuses an existing file and is only reachable from the explicit "Initialize new database" action.
//! - Every connection is checked for identity (DatabaseMetadata.database_id), schema compatibility and
//!   maintenance mode before any operation is submitted.

use serde::Serialize;
use std::path::Path;

/// Shared schema version this app writes. Older databases must be upgraded deliberately
/// (`upgrade_schema`); newer ones are refused for writing.
pub const SHARED_SCHEMA_VERSION: i64 = 1;
/// Protocol version of the operations this app submits. A database may demand a minimum.
pub const CLIENT_PROTOCOL: i64 = 1;
pub const ACCESS_DRIVER: &str = "Microsoft Access Driver (*.mdb, *.accdb)";
pub const DEFAULT_SHARED_PATH: &str =
    r"\\epi-qb-5\VPD_Shared\PgmDocs\Assessment and Coverage\Annual Immunization Status Report\ASR Master List Revamp--2023\asr_email_support.accdb";

// ------------------------------------------------------------------ values and errors

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Text(String),
    Int(i64),
}

impl From<&str> for Val {
    fn from(s: &str) -> Self {
        Val::Text(s.to_string())
    }
}
impl From<String> for Val {
    fn from(s: String) -> Self {
        Val::Text(s)
    }
}
impl From<&String> for Val {
    fn from(s: &String) -> Self {
        Val::Text(s.clone())
    }
}
impl From<i64> for Val {
    fn from(i: i64) -> Self {
        Val::Int(i)
    }
}
impl From<bool> for Val {
    fn from(b: bool) -> Self {
        Val::Int(b as i64)
    }
}
impl<T: Into<Val>> From<Option<T>> for Val {
    fn from(o: Option<T>) -> Self {
        o.map(Into::into).unwrap_or(Val::Null)
    }
}

/// One result row; every column is read as text (numbers as their decimal text).
pub type Row = Vec<Option<String>>;

pub fn col(r: &Row, i: usize) -> Option<String> {
    r.get(i).cloned().flatten()
}
pub fn col_s(r: &Row, i: usize) -> String {
    col(r, i).unwrap_or_default()
}
pub fn col_i(r: &Row, i: usize) -> i64 {
    col(r, i).and_then(|v| v.trim().parse::<f64>().ok()).map(|f| f as i64).unwrap_or(0)
}

/// Plain-language failure classes shown to staff and recorded in Diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrKind {
    /// share/folder not reachable (network, VPN, server down)
    Unavailable,
    /// folder reachable but the database file does not exist
    FileNotFound,
    /// no 64-bit Access ODBC driver
    DriverMissing,
    /// only a 32-bit Access driver is installed; this 64-bit app cannot use it
    ArchMismatch,
    /// another program holds an exclusive lock
    Locked,
    /// Windows permissions do not allow opening the file
    PermissionDenied,
    /// opened, but changes are not allowed (file or folder read-only)
    ReadOnly,
    /// not an ASR shared database, or an unsupported schema
    Incompatible,
    /// an administrator put the database in maintenance mode
    Maintenance,
    /// the file is damaged or not an Access database
    Corrupt,
    /// unique/foreign key rule violated (e.g. two computers inserted the same email)
    Constraint,
    /// connection dropped or timed out mid-operation
    Interrupted,
    /// the database at the path is a different database than the one this computer's queue is bound to
    IdentityChanged,
    Other,
}

impl ErrKind {
    pub fn code(&self) -> &'static str {
        match self {
            ErrKind::Unavailable => "SHARE_UNAVAILABLE",
            ErrKind::FileNotFound => "DB_FILE_NOT_FOUND",
            ErrKind::DriverMissing => "ACCESS_DRIVER_MISSING",
            ErrKind::ArchMismatch => "ACCESS_DRIVER_ARCH_MISMATCH",
            ErrKind::Locked => "DB_LOCKED",
            ErrKind::PermissionDenied => "DB_PERMISSION_DENIED",
            ErrKind::ReadOnly => "DB_READ_ONLY",
            ErrKind::Incompatible => "DB_INCOMPATIBLE",
            ErrKind::Maintenance => "DB_MAINTENANCE",
            ErrKind::Corrupt => "DB_CORRUPT",
            ErrKind::Constraint => "DB_CONSTRAINT",
            ErrKind::Interrupted => "DB_INTERRUPTED",
            ErrKind::IdentityChanged => "DB_IDENTITY_CHANGED",
            ErrKind::Other => "DB_ERROR",
        }
    }
    /// Worth retrying automatically with backoff.
    pub fn transient(&self) -> bool {
        matches!(self, ErrKind::Unavailable | ErrKind::Locked | ErrKind::Interrupted | ErrKind::Constraint)
    }
    pub fn plain(&self) -> &'static str {
        match self {
            ErrKind::Unavailable => "The shared folder cannot be reached. Check the network/VPN connection and that the path is correct. Your changes are saved on this computer and will be sent when the database is reachable.",
            ErrKind::FileNotFound => "The folder is reachable but the database file was not found at this path. Check the path. The app never creates a database there automatically.",
            ErrKind::DriverMissing => "The Microsoft Access database driver (64-bit) is not installed on this computer. Ask IT to install the Microsoft Access Database Engine (64-bit). The app keeps working locally.",
            ErrKind::ArchMismatch => "Only the 32-bit Microsoft Access driver is installed. This app is 64-bit and needs the 64-bit Microsoft Access Database Engine. The app keeps working locally.",
            ErrKind::Locked => "The database is locked by another program or user (for example opened exclusively in Access). The app will retry automatically.",
            ErrKind::PermissionDenied => "Windows denied access to the database file or its folder. Ask for read/write (Modify) permission on the shared folder. The app keeps working locally.",
            ErrKind::ReadOnly => "The database can be read but not changed (read-only file or no write permission on the folder). Shared changes cannot be sent; they stay pending on this computer.",
            ErrKind::Incompatible => "This database is not a compatible ASR Support Desk shared database (different schema or app version).",
            ErrKind::Maintenance => "The shared database is in maintenance mode. Changes stay pending on this computer until maintenance ends.",
            ErrKind::Corrupt => "The database file could not be opened as an Access database (damaged or a different file type).",
            ErrKind::Constraint => "Another computer saved the same record at the same time. The app will retry.",
            ErrKind::Interrupted => "The connection to the shared database was interrupted. The app will retry; nothing was lost.",
            ErrKind::IdentityChanged => "The file at this path is a different shared database than the one this computer was connected to. Nothing was sent. Review it in Settings › Shared Database before switching.",
            ErrKind::Other => "The shared database reported an error.",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedError {
    pub kind: ErrKind,
    /// ODBC SQLSTATE (e.g. "IM002") when available
    pub sqlstate: String,
    /// driver native error code when available
    pub native: i32,
    /// provider message (may contain paths, never credentials; see diag::redact before logging)
    pub message: String,
}

impl std::fmt::Display for SharedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.kind.plain(), self.kind.code())
    }
}

impl SharedError {
    pub fn new(kind: ErrKind, message: impl Into<String>) -> Self {
        SharedError { kind, sqlstate: String::new(), native: 0, message: message.into() }
    }
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
    /// "SQLSTATE IM002 / native -1811" style detail for Diagnostics.
    pub fn provider_detail(&self) -> String {
        let mut v = vec![];
        if !self.sqlstate.is_empty() {
            v.push(format!("SQLSTATE {}", self.sqlstate));
        }
        if self.native != 0 {
            v.push(format!("native {}", self.native));
        }
        v.join(" / ")
    }
}

impl From<SharedError> for crate::error::AppError {
    fn from(e: SharedError) -> Self {
        crate::error::AppError::coded(e.code(), e.to_string())
    }
}

pub type SResult<T> = Result<T, SharedError>;

/// Classify a provider error from its SQLSTATE, native code and message text.
pub fn classify(sqlstate: &str, native: i32, message: &str) -> ErrKind {
    let m = message.to_lowercase();
    if sqlstate == "IM002" || m.contains("data source name not found") {
        return ErrKind::DriverMissing;
    }
    if sqlstate == "23000" || m.contains("duplicate values") || m.contains("unique constraint failed") || m.contains("primary key") && m.contains("constraint") {
        return ErrKind::Constraint;
    }
    if m.contains("already in use") || m.contains("exclusively locked") || m.contains("could not lock") || m.contains("locked by")
        || m.contains("database is locked") || native == -1102 || native == -1032 || native == -1033
    {
        return ErrKind::Locked;
    }
    if m.contains("updateable query") || m.contains("updatable query") || m.contains("read-only") || m.contains("readonly") || m.contains("read only")
        || m.contains("attempt to write a readonly database")
    {
        return ErrKind::ReadOnly;
    }
    if m.contains("permission") || m.contains("access is denied") || m.contains("cannot open or write") || native == -1807 || native == -1809 {
        return ErrKind::PermissionDenied;
    }
    if m.contains("could not find file") || native == -1811 {
        return ErrKind::FileNotFound;
    }
    if m.contains("not a valid path") || m.contains("network") || m.contains("disk or network error") || native == -1023 || native == -1022 {
        return ErrKind::Unavailable;
    }
    if m.contains("unrecognized database format") || m.contains("file is not a database") || m.contains("not a database") {
        return ErrKind::Corrupt;
    }
    if sqlstate.starts_with("08") || m.contains("communication link") || m.contains("timeout") {
        return ErrKind::Interrupted;
    }
    ErrKind::Other
}

// ------------------------------------------------------------------ connection abstraction

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Access,
    Sqlite,
}

impl Dialect {
    /// Column type tokens used in the DDL.
    fn types(&self) -> [(&'static str, &'static str); 3] {
        match self {
            // MEMO = long text; LONG = 32-bit integer; COUNTER = AutoNumber
            Dialect::Access => [("{MEMO}", "MEMO"), ("{LONG}", "LONG"), ("{AUTO}", "COUNTER CONSTRAINT pk_ChangeFeed PRIMARY KEY")],
            // TEXT affinity keeps identifiers with leading zeros as text
            Dialect::Sqlite => [("{MEMO}", "TEXT"), ("{LONG}", "INTEGER"), ("{AUTO}", "INTEGER PRIMARY KEY AUTOINCREMENT")],
        }
    }
    /// `SELECT {TOP}cols FROM ... {LIMIT}` limiting syntax.
    pub fn limit(&self, sql: &str, n: i64) -> String {
        match self {
            Dialect::Access => sql.replacen("SELECT ", &format!("SELECT TOP {n} "), 1),
            Dialect::Sqlite => format!("{sql} LIMIT {n}"),
        }
    }
}

pub trait SharedConn {
    fn dialect(&self) -> Dialect;
    /// Execute a statement; returns affected rows.
    fn exec(&mut self, sql: &str, params: &[Val]) -> SResult<usize>;
    fn query(&mut self, sql: &str, params: &[Val]) -> SResult<Vec<Row>>;
    fn begin(&mut self) -> SResult<()>;
    fn commit(&mut self) -> SResult<()>;
    fn rollback(&mut self) -> SResult<()>;

    fn query_one(&mut self, sql: &str, params: &[Val]) -> SResult<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }
    fn table_exists(&mut self, name: &str) -> SResult<bool>;
}

// ------------------------------------------------------------------ SQLite backend (tests, local simulation)

pub struct SqliteShared {
    pub conn: rusqlite::Connection,
}

impl SqliteShared {
    pub fn open(path: &Path) -> SResult<Self> {
        let conn = rusqlite::Connection::open(path).map_err(sqlite_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(sqlite_err)?;
        Ok(SqliteShared { conn })
    }
    pub fn open_in_memory() -> SResult<Self> {
        Ok(SqliteShared { conn: rusqlite::Connection::open_in_memory().map_err(sqlite_err)? })
    }
}

fn sqlite_err(e: rusqlite::Error) -> SharedError {
    let msg = e.to_string();
    let kind = match &e {
        rusqlite::Error::SqliteFailure(f, _) => match f.code {
            rusqlite::ErrorCode::ConstraintViolation => ErrKind::Constraint,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => ErrKind::Locked,
            rusqlite::ErrorCode::ReadOnly => ErrKind::ReadOnly,
            rusqlite::ErrorCode::CannotOpen => ErrKind::FileNotFound,
            rusqlite::ErrorCode::NotADatabase => ErrKind::Corrupt,
            rusqlite::ErrorCode::PermissionDenied => ErrKind::PermissionDenied,
            _ => classify("", 0, &msg),
        },
        _ => classify("", 0, &msg),
    };
    SharedError { kind, sqlstate: String::new(), native: 0, message: msg }
}

impl rusqlite::ToSql for Val {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value};
        Ok(match self {
            Val::Null => ToSqlOutput::Owned(Value::Null),
            Val::Text(s) => ToSqlOutput::Owned(Value::Text(s.clone())),
            Val::Int(i) => ToSqlOutput::Owned(Value::Integer(*i)),
        })
    }
}

impl SharedConn for SqliteShared {
    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }
    fn exec(&mut self, sql: &str, params: &[Val]) -> SResult<usize> {
        self.conn.execute(sql, rusqlite::params_from_iter(params.iter())).map_err(sqlite_err)
    }
    fn query(&mut self, sql: &str, params: &[Val]) -> SResult<Vec<Row>> {
        let mut st = self.conn.prepare(sql).map_err(sqlite_err)?;
        let n = st.column_count();
        let rows = st
            .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    use rusqlite::types::ValueRef;
                    v.push(match r.get_ref(i)? {
                        ValueRef::Null => None,
                        ValueRef::Integer(i) => Some(i.to_string()),
                        ValueRef::Real(f) => Some(f.to_string()),
                        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).to_string()),
                        ValueRef::Blob(b) => Some(String::from_utf8_lossy(b).to_string()),
                    });
                }
                Ok(v)
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;
        Ok(rows)
    }
    fn begin(&mut self) -> SResult<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE").map_err(sqlite_err)
    }
    fn commit(&mut self) -> SResult<()> {
        self.conn.execute_batch("COMMIT").map_err(sqlite_err)
    }
    fn rollback(&mut self) -> SResult<()> {
        if self.conn.is_autocommit() {
            return Ok(());
        }
        self.conn.execute_batch("ROLLBACK").map_err(sqlite_err)
    }
    fn table_exists(&mut self, name: &str) -> SResult<bool> {
        Ok(!self.query("SELECT name FROM sqlite_master WHERE type='table' AND name=?", &[name.into()])?.is_empty())
    }
}

// ------------------------------------------------------------------ Access backend (ODBC)

mod access {
    use super::*;
    use odbc_api::parameter::{InputParameter, VarWCharBox, WithDataType};
    use odbc_api::{ConnectionOptions, Cursor, Environment, ResultSetMetadata};
    use std::sync::OnceLock;

    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();

    fn env() -> SResult<&'static Environment> {
        match ENV.get_or_init(|| Environment::new().map_err(|e| e.to_string())) {
            Ok(e) => Ok(e),
            Err(m) => Err(SharedError::new(ErrKind::DriverMissing, format!("ODBC environment could not be created: {m}"))),
        }
    }

    pub(super) fn map_err(e: odbc_api::Error) -> SharedError {
        let (state, native) = match &e {
            odbc_api::Error::Diagnostics { record, .. } => (record.state.as_str().to_string(), record.native_error),
            _ => (String::new(), 0),
        };
        let full = e.to_string();
        // drop the "ODBC emitted an error calling 'X':" prefix; keep the provider text
        let message = full.split_once(":\n").map(|(_, m)| m.to_string()).unwrap_or(full);
        SharedError { kind: classify(&state, native, &message), sqlstate: state, native, message }
    }

    pub struct AccessConn {
        conn: odbc_api::Connection<'static>,
    }

    /// `DBQ` value: braces only when needed (`;`, `{`, `}`), with `}` doubled per ODBC rules.
    pub(super) fn connection_string(path: &Path, read_only: bool) -> String {
        let p = path.to_string_lossy();
        let dbq = if p.contains([';', '{', '}']) { format!("{{{}}}", p.replace('}', "}}")) } else { p.to_string() };
        format!("Driver={{{ACCESS_DRIVER}}};DBQ={dbq};{}", if read_only { "ReadOnly=1;" } else { "" })
    }

    impl AccessConn {
        pub fn open(path: &Path, read_only: bool) -> SResult<Self> {
            Self::open_with(path, read_only, "")
        }

        /// `extra` = additional driver keywords (tests use "Exclusive=1;" to simulate another user's lock).
        pub fn open_with(path: &Path, read_only: bool, extra: &str) -> SResult<Self> {
            let env = env()?;
            let cs = format!("{}{extra}", connection_string(path, read_only));
            let conn = env.connect_with_connection_string(&cs, ConnectionOptions::default()).map_err(map_err)?;
            Ok(AccessConn { conn })
        }

        fn params(params: &[Val]) -> Vec<Box<dyn InputParameter>> {
            params
                .iter()
                .map(|p| -> Box<dyn InputParameter> {
                    // The Access driver rejects (HY104 "Invalid precision value") text parameters whose declared
                    // size is 0 (empty strings, NULL) or over 255 characters unless bound as long text
                    // (SQL_WLONGVARCHAR = MEMO). The declared SQL type is therefore always set explicitly.
                    let one = std::num::NonZeroUsize::new(1);
                    match p {
                        Val::Null => Box::new(WithDataType::new(VarWCharBox::null(), odbc_api::DataType::WVarchar { length: one })),
                        Val::Text(s) => {
                            let n = s.encode_utf16().count();
                            let dt = if n > 255 {
                                odbc_api::DataType::WLongVarchar { length: std::num::NonZeroUsize::new(n) }
                            } else {
                                odbc_api::DataType::WVarchar { length: std::num::NonZeroUsize::new(n.max(1)) }
                            };
                            Box::new(WithDataType::new(VarWCharBox::from_str_slice(s), dt))
                        }
                        Val::Int(i) => Box::new((*i).clamp(i32::MIN as i64, i32::MAX as i64) as i32),
                    }
                })
                .collect()
        }
    }

    impl SharedConn for AccessConn {
        fn dialect(&self) -> Dialect {
            Dialect::Access
        }
        fn exec(&mut self, sql: &str, params: &[Val]) -> SResult<usize> {
            let ps = Self::params(params);
            let mut stmt = self.conn.preallocate().map_err(map_err)?;
            {
                let _cursor = stmt.execute(sql, ps.as_slice()).map_err(map_err)?;
            }
            Ok(stmt.row_count().map_err(map_err)?.unwrap_or(0))
        }
        fn query(&mut self, sql: &str, params: &[Val]) -> SResult<Vec<Row>> {
            let ps = Self::params(params);
            let mut out = Vec::new();
            if let Some(mut cursor) = self.conn.execute(sql, ps.as_slice(), None).map_err(map_err)? {
                let n = cursor.num_result_cols().map_err(map_err)?.max(0) as u16;
                let mut buf: Vec<u16> = Vec::with_capacity(256);
                while let Some(mut row) = cursor.next_row().map_err(map_err)? {
                    let mut r: Row = Vec::with_capacity(n as usize);
                    for c in 1..=n {
                        let present = row.get_wide_text(c, &mut buf).map_err(map_err)?;
                        r.push(if present { Some(String::from_utf16_lossy(&buf)) } else { None });
                    }
                    out.push(r);
                }
            }
            Ok(out)
        }
        fn begin(&mut self) -> SResult<()> {
            self.conn.set_autocommit(false).map_err(map_err)
        }
        fn commit(&mut self) -> SResult<()> {
            let r = self.conn.commit().map_err(map_err);
            let _ = self.conn.set_autocommit(true);
            r
        }
        fn rollback(&mut self) -> SResult<()> {
            let r = self.conn.rollback().map_err(map_err);
            let _ = self.conn.set_autocommit(true);
            r
        }
        fn table_exists(&mut self, name: &str) -> SResult<bool> {
            // SQLTables via the catalog API: robust on Access (MSysObjects is not readable by default)
            let stmt = self.conn.preallocate().map_err(map_err)?;
            let mut cursor = stmt.into_tables_cursor("", "", name, "TABLE").map_err(map_err)?;
            let mut found = false;
            let mut buf = Vec::new();
            while let Some(mut row) = cursor.next_row().map_err(map_err)? {
                if row.get_text(3, &mut buf).map_err(map_err)? && String::from_utf8_lossy(&buf).eq_ignore_ascii_case(name) {
                    found = true;
                }
            }
            Ok(found)
        }
    }

    /// Create a new, empty .accdb with the ODBC installer API (CREATE_DBV12). Refuses existing files.
    pub fn create_file(path: &Path) -> SResult<()> {
        #[link(name = "odbccp32", kind = "raw-dylib")]
        extern "system" {
            fn SQLConfigDataSourceW(hwnd: *mut std::ffi::c_void, request: u16, driver: *const u16, attributes: *const u16) -> i32;
            fn SQLInstallerErrorW(i: u16, err: *mut u32, msg: *mut u16, max: u16, out: *mut u16) -> i16;
        }
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let driver = wide(ACCESS_DRIVER);
        for kw in ["CREATE_DBV12", "CREATE_DB"] {
            // attributes are NUL-separated and end with a double NUL
            let mut attrs: Vec<u16> = format!("{kw}=\"{}\" General", path.display()).encode_utf16().collect();
            attrs.extend([0, 0]);
            let ok = unsafe { SQLConfigDataSourceW(std::ptr::null_mut(), 1, driver.as_ptr(), attrs.as_ptr()) };
            if ok != 0 && path.exists() {
                return Ok(());
            }
        }
        let mut code: u32 = 0;
        let mut buf = vec![0u16; 512];
        let mut len: u16 = 0;
        unsafe { SQLInstallerErrorW(1, &mut code, buf.as_mut_ptr(), buf.len() as u16, &mut len) };
        let msg = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
        let kind = if code == 6 || msg.to_lowercase().contains("driver") { ErrKind::DriverMissing } else { classify("", 0, &msg) };
        Err(SharedError::new(kind, format!("Could not create the database file (installer error {code}): {msg}")))
    }
}

pub use access::AccessConn;

/// Open the shared database at `path`: Access for .accdb/.mdb, SQLite otherwise (test simulations only).
pub fn open(path: &Path, read_only: bool) -> SResult<Box<dyn SharedConn>> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "accdb" || ext == "mdb" {
        match AccessConn::open(path, read_only) {
            Ok(c) => Ok(Box::new(c)),
            Err(mut e) if e.kind == ErrKind::DriverMissing => {
                if driver_architecture() == DriverArch::Only32Bit {
                    e.kind = ErrKind::ArchMismatch;
                }
                Err(e)
            }
            Err(e) => Err(e),
        }
    } else {
        if !path.exists() {
            return Err(SharedError::new(ErrKind::FileNotFound, "database file not found"));
        }
        let c = SqliteShared::open(path)?;
        if read_only {
            c.conn.execute_batch("PRAGMA query_only=1").map_err(sqlite_err)?;
        }
        Ok(Box::new(c))
    }
}

pub fn connection_string_redacted(path: &Path) -> String {
    // the connection string holds only driver + path (no credentials); paths are shown in Settings anyway
    access::connection_string(path, false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverArch {
    Available64,
    Only32Bit,
    None,
    Unknown,
}

/// Which Access ODBC drivers are registered (registry lookup through reg.exe; no extra dependency).
pub fn driver_architecture() -> DriverArch {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let q = |key: &str| {
            std::process::Command::new("reg")
                .args(["query", key, "/v", "Driver"])
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .output()
                .map(|o| o.status.success())
                .ok()
        };
        let k64 = q(&format!(r"HKLM\SOFTWARE\ODBC\ODBCINST.INI\{ACCESS_DRIVER}"));
        let k32 = q(&format!(r"HKLM\SOFTWARE\WOW6432Node\ODBC\ODBCINST.INI\{ACCESS_DRIVER}"));
        match (k64, k32) {
            (Some(true), _) => DriverArch::Available64,
            (Some(false), Some(true)) => DriverArch::Only32Bit,
            (Some(false), Some(false)) => DriverArch::None,
            _ => DriverArch::Unknown,
        }
    }
    #[cfg(not(windows))]
    {
        DriverArch::Unknown
    }
}

// ------------------------------------------------------------------ schema

/// Shared schema v1. Statements are separated by `;;`. No ON DELETE CASCADE anywhere: case history is
/// never deleted; deletions are markers (is_deleted) and duplicates are redirects (TicketAliases).
const SCHEMA_V1: &str = r#"
CREATE TABLE DatabaseMetadata (meta_key TEXT(64) NOT NULL CONSTRAINT pk_DatabaseMetadata PRIMARY KEY, meta_value {MEMO});;
CREATE TABLE SharedSettings (setting_key TEXT(64) NOT NULL CONSTRAINT pk_SharedSettings PRIMARY KEY, setting_value {MEMO}, row_version {LONG} NOT NULL, updated_at TEXT(30), updated_by TEXT(100));;
CREATE TABLE Staff (staff_id TEXT(36) NOT NULL CONSTRAINT pk_Staff PRIMARY KEY, display_name TEXT(100) NOT NULL, email TEXT(254), windows_account TEXT(128), is_active {LONG} NOT NULL, row_version {LONG} NOT NULL, created_at TEXT(30) NOT NULL, updated_at TEXT(30) NOT NULL);;
CREATE TABLE Devices (device_id TEXT(36) NOT NULL CONSTRAINT pk_Devices PRIMARY KEY, device_name TEXT(128), windows_account TEXT(128), app_version TEXT(32), first_seen_at TEXT(30), last_seen_at TEXT(30));;
CREATE TABLE Mailboxes (mailbox_id TEXT(36) NOT NULL CONSTRAINT pk_Mailboxes PRIMARY KEY, logical_key TEXT(255) NOT NULL, display_name TEXT(255), created_at TEXT(30));;
CREATE UNIQUE INDEX ux_Mailboxes_key ON Mailboxes (logical_key);;
CREATE TABLE Tickets (ticket_id TEXT(36) NOT NULL CONSTRAINT pk_Tickets PRIMARY KEY, ticket_ref TEXT(32) NOT NULL, subject TEXT(255), sender_name TEXT(255), sender_email TEXT(254), school_name TEXT(255), njiis_id TEXT(64), priority TEXT(16), ticket_status TEXT(16) NOT NULL, owner_staff_id TEXT(36), review_reason TEXT(255), staff_actions {MEMO}, first_received_at TEXT(30), last_incoming_at TEXT(30), done_at TEXT(30), merged_into TEXT(36), row_version {LONG} NOT NULL, created_at TEXT(30) NOT NULL, updated_at TEXT(30) NOT NULL, created_by_device TEXT(36), origin TEXT(16));;
CREATE UNIQUE INDEX ux_Tickets_ref ON Tickets (ticket_ref);;
CREATE INDEX ix_Tickets_status ON Tickets (ticket_status);;
CREATE INDEX ix_Tickets_owner ON Tickets (owner_staff_id);;
CREATE TABLE TicketAliases (alias_key TEXT(64) NOT NULL CONSTRAINT pk_TicketAliases PRIMARY KEY, ticket_id TEXT(36) NOT NULL, alias_type TEXT(16) NOT NULL, created_at TEXT(30) NOT NULL, CONSTRAINT fk_TicketAliases_ticket FOREIGN KEY (ticket_id) REFERENCES Tickets (ticket_id));;
CREATE TABLE Emails (email_id TEXT(36) NOT NULL CONSTRAINT pk_Emails PRIMARY KEY, match_key TEXT(80) NOT NULL, match_method TEXT(16) NOT NULL, evidence_key TEXT(80), mailbox_id TEXT(36), internet_message_id TEXT(255), conversation_id TEXT(255), subject TEXT(255), sender_name TEXT(255), sender_email TEXT(254), received_at TEXT(30), is_internal {LONG} NOT NULL, is_auto_reply {LONG} NOT NULL, body_text {MEMO}, body_truncated {LONG} NOT NULL, created_at TEXT(30) NOT NULL, created_by_device TEXT(36));;
CREATE UNIQUE INDEX ux_Emails_match ON Emails (match_key);;
CREATE INDEX ix_Emails_evidence ON Emails (evidence_key);;
CREATE INDEX ix_Emails_conversation ON Emails (conversation_id);;
CREATE TABLE TicketEmails (ticket_id TEXT(36) NOT NULL, email_id TEXT(36) NOT NULL, link_type TEXT(16) NOT NULL, is_active {LONG} NOT NULL, linked_at TEXT(30) NOT NULL, linked_by_device TEXT(36), CONSTRAINT pk_TicketEmails PRIMARY KEY (ticket_id, email_id), CONSTRAINT fk_TicketEmails_ticket FOREIGN KEY (ticket_id) REFERENCES Tickets (ticket_id), CONSTRAINT fk_TicketEmails_email FOREIGN KEY (email_id) REFERENCES Emails (email_id));;
CREATE INDEX ix_TicketEmails_email ON TicketEmails (email_id);;
CREATE TABLE TicketIssueTypes (ticket_id TEXT(36) NOT NULL, issue_type TEXT(64) NOT NULL, sort_order {LONG} NOT NULL, origin TEXT(16) NOT NULL, CONSTRAINT pk_TicketIssueTypes PRIMARY KEY (ticket_id, issue_type), CONSTRAINT fk_TicketIssueTypes_ticket FOREIGN KEY (ticket_id) REFERENCES Tickets (ticket_id));;
CREATE TABLE TicketSchools (ticket_id TEXT(36) NOT NULL, slot_no {LONG} NOT NULL, is_removed {LONG} NOT NULL, CONSTRAINT pk_TicketSchools PRIMARY KEY (ticket_id, slot_no), CONSTRAINT fk_TicketSchools_ticket FOREIGN KEY (ticket_id) REFERENCES Tickets (ticket_id));;
CREATE TABLE TicketFields (ticket_id TEXT(36) NOT NULL, field_key TEXT(64) NOT NULL, extracted_value {MEMO}, manual_value {MEMO}, has_manual {LONG} NOT NULL, is_verified {LONG} NOT NULL, row_version {LONG} NOT NULL, updated_at TEXT(30), updated_by TEXT(100), CONSTRAINT pk_TicketFields PRIMARY KEY (ticket_id, field_key), CONSTRAINT fk_TicketFields_ticket FOREIGN KEY (ticket_id) REFERENCES Tickets (ticket_id));;
CREATE TABLE TicketNotes (note_id TEXT(36) NOT NULL CONSTRAINT pk_TicketNotes PRIMARY KEY, ticket_id TEXT(36) NOT NULL, author_staff_id TEXT(36), author_name TEXT(100), note_text {MEMO}, row_version {LONG} NOT NULL, is_deleted {LONG} NOT NULL, created_at TEXT(30) NOT NULL, updated_at TEXT(30) NOT NULL, device_id TEXT(36), CONSTRAINT fk_TicketNotes_ticket FOREIGN KEY (ticket_id) REFERENCES Tickets (ticket_id));;
CREATE INDEX ix_TicketNotes_ticket ON TicketNotes (ticket_id);;
CREATE TABLE OwnershipHistory (entry_id TEXT(36) NOT NULL CONSTRAINT pk_OwnershipHistory PRIMARY KEY, ticket_id TEXT(36) NOT NULL, staff_id TEXT(36) NOT NULL, started_at TEXT(30) NOT NULL, ended_at TEXT(30), started_by TEXT(100), ended_by TEXT(100), start_reason TEXT(64), end_reason TEXT(64), op_id TEXT(36), CONSTRAINT fk_OwnershipHistory_ticket FOREIGN KEY (ticket_id) REFERENCES Tickets (ticket_id));;
CREATE INDEX ix_OwnershipHistory_ticket ON OwnershipHistory (ticket_id);;
CREATE TABLE CompletionHistory (completion_id TEXT(36) NOT NULL CONSTRAINT pk_CompletionHistory PRIMARY KEY, ticket_id TEXT(36) NOT NULL, actor_staff_id TEXT(36), actor_name TEXT(100), decided_at TEXT(30) NOT NULL, accepted_at TEXT(30), covered_email_ids {MEMO}, max_covered_received_at TEXT(30), op_id TEXT(36), completion_state TEXT(16) NOT NULL, state_reason TEXT(255), CONSTRAINT fk_CompletionHistory_ticket FOREIGN KEY (ticket_id) REFERENCES Tickets (ticket_id));;
CREATE INDEX ix_CompletionHistory_ticket ON CompletionHistory (ticket_id);;
CREATE TABLE ActivityLog (activity_id TEXT(36) NOT NULL CONSTRAINT pk_ActivityLog PRIMARY KEY, ticket_id TEXT(36) NOT NULL, occurred_at TEXT(30) NOT NULL, activity_source TEXT(16) NOT NULL, actor_staff_id TEXT(36), actor_name TEXT(100), device_id TEXT(36), op_id TEXT(36), action_text TEXT(255) NOT NULL, field_name TEXT(64), old_value {MEMO}, new_value {MEMO}, CONSTRAINT fk_ActivityLog_ticket FOREIGN KEY (ticket_id) REFERENCES Tickets (ticket_id));;
CREATE INDEX ix_ActivityLog_ticket ON ActivityLog (ticket_id);;
CREATE TABLE ProcessedOperations (op_id TEXT(36) NOT NULL CONSTRAINT pk_ProcessedOperations PRIMARY KEY, device_id TEXT(36), staff_id TEXT(36), op_type TEXT(32) NOT NULL, ticket_id TEXT(36), processed_at TEXT(30) NOT NULL, op_result TEXT(16) NOT NULL, result_json {MEMO});;
CREATE TABLE SyncConflicts (conflict_id TEXT(36) NOT NULL CONSTRAINT pk_SyncConflicts PRIMARY KEY, op_id TEXT(36) NOT NULL, ticket_id TEXT(36), device_id TEXT(36), staff_id TEXT(36), staff_name TEXT(100), field_name TEXT(64), conflict_type TEXT(32) NOT NULL, original_value {MEMO}, proposed_value {MEMO}, shared_value {MEMO}, reason TEXT(255), conflict_status TEXT(16) NOT NULL, resolution_kind TEXT(32), created_at TEXT(30) NOT NULL, resolved_at TEXT(30), resolved_by TEXT(100));;
CREATE INDEX ix_SyncConflicts_ticket ON SyncConflicts (ticket_id);;
CREATE TABLE TicketRelations (relation_id TEXT(36) NOT NULL CONSTRAINT pk_TicketRelations PRIMARY KEY, ticket_a TEXT(36) NOT NULL, ticket_b TEXT(36) NOT NULL, relation_type TEXT(32) NOT NULL, evidence {MEMO}, relation_status TEXT(16) NOT NULL, created_at TEXT(30) NOT NULL, created_by_device TEXT(36));;
CREATE TABLE ChangeFeed (seq {AUTO}, ticket_id TEXT(36), entity_type TEXT(16) NOT NULL, entity_id TEXT(64), ticket_version {LONG}, op_id TEXT(36), recorded_at TEXT(30) NOT NULL);;
CREATE INDEX ix_ChangeFeed_ticket ON ChangeFeed (ticket_id)
"#;

/// Per-version DDL steps; index = target version - 1.
const SCHEMA_STEPS: &[&str] = &[SCHEMA_V1];

fn ddl(d: Dialect, block: &str) -> Vec<String> {
    let mut s = block.to_string();
    for (tok, t) in d.types() {
        s = s.replace(tok, t);
    }
    s.split(";;").map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DbIdentity {
    pub database_id: String,
    pub display_name: String,
    pub environment: String,
    pub schema_version: i64,
    pub min_client_protocol: i64,
    pub maintenance: bool,
    pub maintenance_message: String,
    pub created_at: String,
}

pub fn read_meta(c: &mut dyn SharedConn) -> SResult<std::collections::HashMap<String, String>> {
    let rows = c.query("SELECT meta_key, meta_value FROM DatabaseMetadata", &[])?;
    Ok(rows.into_iter().map(|r| (col_s(&r, 0), col_s(&r, 1))).collect())
}

/// Read the identity of a connected database. Errors with `Incompatible` if it is not an ASR database.
pub fn identity(c: &mut dyn SharedConn) -> SResult<DbIdentity> {
    if !c.table_exists("DatabaseMetadata")? {
        return Err(SharedError::new(ErrKind::Incompatible, "The file has no DatabaseMetadata table: it is not an ASR Support Desk shared database (or it was never initialized)."));
    }
    let m = read_meta(c)?;
    let g = |k: &str| m.get(k).cloned().unwrap_or_default();
    if g("database_id").is_empty() {
        return Err(SharedError::new(ErrKind::Incompatible, "DatabaseMetadata has no database_id."));
    }
    Ok(DbIdentity {
        database_id: g("database_id"),
        display_name: g("display_name"),
        environment: g("environment"),
        schema_version: g("schema_version").parse().unwrap_or(0),
        min_client_protocol: g("min_client_protocol").parse().unwrap_or(1),
        maintenance: g("maintenance") == "1",
        maintenance_message: g("maintenance_message"),
        created_at: g("created_at"),
    })
}

/// "ok" | "upgrade_required" (db older; an admin must upgrade) | "app_too_old" (db newer or demands a newer protocol)
pub fn compatibility(id: &DbIdentity) -> &'static str {
    if id.schema_version > SHARED_SCHEMA_VERSION || id.min_client_protocol > CLIENT_PROTOCOL {
        "app_too_old"
    } else if id.schema_version < SHARED_SCHEMA_VERSION {
        "upgrade_required"
    } else {
        "ok"
    }
}

/// Create the schema in an EMPTY database (no tables) and write a new identity.
/// Refuses databases that already contain any of the ASR tables.
pub fn initialize_schema(c: &mut dyn SharedConn, display_name: &str, environment: &str, created_by: &str) -> SResult<DbIdentity> {
    for t in ["DatabaseMetadata", "Tickets", "Emails"] {
        if c.table_exists(t)? {
            return Err(SharedError::new(ErrKind::Incompatible, "This database already contains ASR Support Desk tables. It was not changed."));
        }
    }
    let d = c.dialect();
    for step in SCHEMA_STEPS {
        for stmt in ddl(d, step) {
            c.exec(&stmt, &[])?;
        }
    }
    let ts = crate::db::now();
    let id = uuid::Uuid::new_v4().to_string();
    c.begin()?;
    let meta: [(&str, String); 8] = [
        ("database_id", id.clone()),
        ("display_name", display_name.to_string()),
        ("environment", environment.to_string()),
        ("schema_version", SHARED_SCHEMA_VERSION.to_string()),
        ("min_client_protocol", CLIENT_PROTOCOL.to_string()),
        ("maintenance", "0".into()),
        ("created_at", ts.clone()),
        ("created_by", created_by.to_string()),
    ];
    for (k, v) in meta.iter() {
        if let Err(e) = c.exec("INSERT INTO DatabaseMetadata (meta_key, meta_value) VALUES (?, ?)", &[(*k).into(), v.into()]) {
            let _ = c.rollback();
            return Err(e);
        }
    }
    c.commit()?;
    identity(c)
}

/// Controlled schema upgrade (one process, maintenance flag set first). The caller must have made a file
/// backup of the database first (`backup_file`). Steps already applied are skipped; restartable.
pub fn upgrade_schema(c: &mut dyn SharedConn, actor: &str) -> SResult<DbIdentity> {
    let id = identity(c)?;
    if id.schema_version >= SHARED_SCHEMA_VERSION {
        return Ok(id);
    }
    set_meta(c, "maintenance", "1")?;
    set_meta(c, "maintenance_message", &format!("Schema upgrade by {actor}"))?;
    let d = c.dialect();
    for v in (id.schema_version as usize)..SCHEMA_STEPS.len() {
        for stmt in ddl(d, SCHEMA_STEPS[v]) {
            c.exec(&stmt, &[])?;
        }
        set_meta(c, "schema_version", &(v as i64 + 1).to_string())?;
    }
    set_meta(c, "maintenance", "0")?;
    set_meta(c, "maintenance_message", "")?;
    identity(c)
}

pub fn set_meta(c: &mut dyn SharedConn, key: &str, value: &str) -> SResult<()> {
    if c.exec("UPDATE DatabaseMetadata SET meta_value = ? WHERE meta_key = ?", &[value.into(), key.into()])? == 0 {
        c.exec("INSERT INTO DatabaseMetadata (meta_key, meta_value) VALUES (?, ?)", &[key.into(), value.into()])?;
    }
    Ok(())
}

/// Copy the database file next to itself as `<name>.backup-<time>.accdb` before an upgrade.
pub fn backup_file(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "shared".into());
    let ext = path.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "accdb".into());
    let dest = path.with_file_name(format!("{stem}.backup-{}.{ext}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    std::fs::copy(path, &dest)?;
    Ok(dest)
}

/// Create a NEW database file at `path` (must not exist) and initialize it. This is the deliberate
/// setup/test path; it never touches an existing file.
pub fn create_database(path: &Path, display_name: &str, environment: &str, created_by: &str) -> SResult<DbIdentity> {
    if path.exists() {
        return Err(SharedError::new(ErrKind::Incompatible, "A file already exists at this path. The app never overwrites an existing database; choose a new file name."));
    }
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() && !p.exists() => {
            return Err(SharedError::new(ErrKind::Unavailable, "The folder for the new database does not exist or cannot be reached."));
        }
        _ => {}
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "accdb" || ext == "mdb" {
        access::create_file(path)?;
    } else {
        SqliteShared::open(path)?;
    }
    let mut c = open(path, false)?;
    initialize_schema(c.as_mut(), display_name, environment, created_by)
}

// ------------------------------------------------------------------ connection check

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProbeReport {
    /// the folder/share answered
    pub reachable: bool,
    /// the database opened and the metadata could be read
    pub readable: bool,
    /// a write (device heartbeat) was committed
    pub writable: bool,
    pub identity: Option<DbIdentity>,
    /// "ok" | "upgrade_required" | "app_too_old" | ""
    pub compatibility: String,
    pub driver: Option<DriverArch>,
    pub error: Option<SharedError>,
    pub elapsed_ms: u64,
}

/// Check the database: reachable → readable (identity) → writable (heartbeat). Never creates anything at
/// `path`. Blocking; call from a background thread.
pub fn probe(path: &Path, device: &DeviceInfo) -> ProbeReport {
    let started = std::time::Instant::now();
    let mut rep = ProbeReport::default();
    let fin = |mut r: ProbeReport| {
        r.elapsed_ms = started.elapsed().as_millis() as u64;
        r
    };
    // 1. folder / file reachability without the driver (fast, and distinguishes share vs. file problems)
    let parent_ok = path.parent().map(|p| p.as_os_str().is_empty() || p.exists()).unwrap_or(true);
    if !parent_ok {
        rep.error = Some(SharedError::new(ErrKind::Unavailable, "The folder of the database path cannot be reached."));
        return fin(rep);
    }
    rep.reachable = true;
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            rep.error = Some(SharedError::new(ErrKind::FileNotFound, "The database file does not exist at this path."));
            return fin(rep);
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            rep.error = Some(SharedError::new(ErrKind::PermissionDenied, e.to_string()));
            return fin(rep);
        }
        _ => {}
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "accdb" || ext == "mdb" {
        rep.driver = Some(driver_architecture());
    }
    // 2. open + identity
    let mut c = match open(path, false) {
        Ok(c) => c,
        Err(e) if matches!(e.kind, ErrKind::ReadOnly | ErrKind::PermissionDenied) => {
            // try a read-only connection to tell "read-only" from "no access at all"
            match open(path, true) {
                Ok(mut ro) => match identity(ro.as_mut()) {
                    Ok(id) => {
                        rep.readable = true;
                        rep.compatibility = compatibility(&id).into();
                        rep.identity = Some(id);
                        rep.error = Some(SharedError { kind: ErrKind::ReadOnly, ..e });
                        return fin(rep);
                    }
                    Err(e2) => {
                        rep.error = Some(e2);
                        return fin(rep);
                    }
                },
                Err(_) => {
                    rep.error = Some(e);
                    return fin(rep);
                }
            }
        }
        Err(e) => {
            rep.error = Some(e);
            return fin(rep);
        }
    };
    let id = match identity(c.as_mut()) {
        Ok(id) => id,
        Err(e) => {
            rep.error = Some(e);
            return fin(rep);
        }
    };
    rep.readable = true;
    rep.compatibility = compatibility(&id).into();
    rep.identity = Some(id.clone());
    if rep.compatibility != "ok" {
        rep.error = Some(SharedError::new(
            ErrKind::Incompatible,
            if rep.compatibility == "upgrade_required" {
                format!("The shared database uses schema v{} and this app needs v{}. An administrator must run the controlled upgrade.", id.schema_version, SHARED_SCHEMA_VERSION)
            } else {
                format!("The shared database (schema v{}, protocol {}) needs a newer version of this app.", id.schema_version, id.min_client_protocol)
            },
        ));
        return fin(rep);
    }
    // 3. write check: device heartbeat in its own short transaction
    match heartbeat(c.as_mut(), device) {
        Ok(()) => rep.writable = true,
        Err(e) => {
            let kind = if matches!(e.kind, ErrKind::ReadOnly | ErrKind::PermissionDenied) { ErrKind::ReadOnly } else { e.kind };
            rep.error = Some(SharedError { kind, ..e });
            return fin(rep);
        }
    }
    if id.maintenance {
        rep.error = Some(SharedError::new(ErrKind::Maintenance, id.maintenance_message.clone()));
    }
    fin(rep)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub windows_account: String,
    pub app_version: String,
}

pub fn heartbeat(c: &mut dyn SharedConn, d: &DeviceInfo) -> SResult<()> {
    let ts = crate::db::now();
    c.begin()?;
    let r = (|| {
        let n = c.exec(
            "UPDATE Devices SET device_name = ?, windows_account = ?, app_version = ?, last_seen_at = ? WHERE device_id = ?",
            &[(&d.device_name).into(), (&d.windows_account).into(), (&d.app_version).into(), (&ts).into(), (&d.device_id).into()],
        )?;
        if n == 0 {
            c.exec(
                "INSERT INTO Devices (device_id, device_name, windows_account, app_version, first_seen_at, last_seen_at) VALUES (?, ?, ?, ?, ?, ?)",
                &[(&d.device_id).into(), (&d.device_name).into(), (&d.windows_account).into(), (&d.app_version).into(), (&ts).into(), (&ts).into()],
            )?;
        }
        Ok(())
    })();
    match r {
        Ok(()) => c.commit(),
        Err(e) => {
            let _ = c.rollback();
            Err(e)
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A new isolated shared database for one test. SQLite by default; with
    /// ASR_SHARED_BACKEND=access and ASR_ACCESS_TEST_DIR set, a NEW .accdb through the real Access driver.
    pub fn new_shared(dir: &Path) -> std::path::PathBuf {
        let access = std::env::var("ASR_SHARED_BACKEND").map(|v| v == "access").unwrap_or(false);
        let p = match (access, std::env::var("ASR_ACCESS_TEST_DIR")) {
            (true, Ok(d)) => std::path::PathBuf::from(d).join(format!("asr-suite-{}.accdb", uuid::Uuid::new_v4())),
            _ => dir.join("shared.sqlite"),
        };
        create_database(&p, "Test shared DB", "test", "tests").unwrap();
        p
    }

    pub fn device(n: &str) -> DeviceInfo {
        DeviceInfo { device_id: format!("dev-{n}"), device_name: format!("PC-{n}"), windows_account: format!("NJ\\{n}"), app_version: "test".into() }
    }

    #[test]
    fn create_refuses_existing_file_and_identity_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let p = new_shared(dir.path());
        let e = create_database(&p, "x", "test", "t").unwrap_err();
        assert_eq!(e.kind, ErrKind::Incompatible, "existing files are never overwritten");
        let mut c = open(&p, false).unwrap();
        let a = identity(c.as_mut()).unwrap();
        let b = identity(c.as_mut()).unwrap();
        assert_eq!(a.database_id, b.database_id);
        assert_eq!(compatibility(&a), "ok");
        assert!(initialize_schema(c.as_mut(), "again", "test", "t").is_err(), "an initialized database is not re-initialized");
    }

    #[test]
    fn probe_distinguishes_missing_file_folder_readonly_and_writable() {
        let dir = tempfile::tempdir().unwrap();
        let d = device("A");
        let r = probe(&dir.path().join("missing.sqlite"), &d);
        assert!(r.reachable && !r.readable);
        assert_eq!(r.error.unwrap().kind, ErrKind::FileNotFound);
        assert!(!dir.path().join("missing.sqlite").exists(), "probe never creates a database");
        let r = probe(&dir.path().join("nofolder").join("x.sqlite"), &d);
        assert!(!r.reachable);
        assert_eq!(r.error.unwrap().kind, ErrKind::Unavailable);
        let p = new_shared(dir.path());
        let r = probe(&p, &d);
        assert!(r.reachable && r.readable && r.writable, "{:?}", r.error);
        // read-only file: readable but not writable
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&p, perm.clone()).unwrap();
        let r = probe(&p, &device("B"));
        perm.set_readonly(false);
        std::fs::set_permissions(&p, perm).unwrap();
        assert!(r.readable, "{:?}", r.error);
        assert!(!r.writable);
        assert_eq!(r.error.unwrap().kind, ErrKind::ReadOnly);
    }

    #[test]
    fn not_an_asr_database_is_incompatible() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("other.sqlite");
        rusqlite::Connection::open(&p).unwrap().execute_batch("CREATE TABLE x(a)").unwrap();
        let r = probe(&p, &device("A"));
        assert!(r.reachable && !r.readable);
        assert_eq!(r.error.unwrap().kind, ErrKind::Incompatible);
    }

    #[test]
    fn newer_schema_blocks_writes_and_upgrade_is_controlled() {
        let dir = tempfile::tempdir().unwrap();
        let p = new_shared(dir.path());
        {
            let mut c = open(&p, false).unwrap();
            set_meta(c.as_mut(), "schema_version", "99").unwrap();
        }
        let r = probe(&p, &device("A"));
        assert_eq!(r.compatibility, "app_too_old");
        assert!(!r.writable, "incompatible writers are blocked before any write");
        {
            let mut c = open(&p, false).unwrap();
            set_meta(c.as_mut(), "schema_version", "0").unwrap();
        }
        let r = probe(&p, &device("A"));
        assert_eq!(r.compatibility, "upgrade_required");
        // simulate a v0 database that is missing nothing (DDL already present) -> upgrade must not destroy data
        let mut c = open(&p, false).unwrap();
        c.exec("INSERT INTO Staff (staff_id, display_name, is_active, row_version, created_at, updated_at) VALUES ('s1','Aaron',1,1,'x','x')", &[]).unwrap();
        // re-running DDL for v1 on an existing v1 layout fails; the upgrade aborts and leaves maintenance on
        assert!(upgrade_schema(c.as_mut(), "admin").is_err());
        assert_eq!(identity(c.as_mut()).unwrap().maintenance, true, "a failed upgrade leaves the database in maintenance (writers blocked)");
        let n = c.query("SELECT COUNT(*) FROM Staff", &[]).unwrap();
        assert_eq!(col_i(&n[0], 0), 1, "data kept");
    }

    #[test]
    fn classify_provider_messages() {
        assert_eq!(classify("IM002", 0, "[Microsoft][ODBC Driver Manager] Data source name not found and no default driver specified"), ErrKind::DriverMissing);
        assert_eq!(classify("HY000", -1811, "[Microsoft][ODBC Microsoft Access Driver] Could not find file 'x.accdb'."), ErrKind::FileNotFound);
        assert_eq!(classify("HY000", -1023, "'\\\\srv\\x' is not a valid path."), ErrKind::Unavailable);
        assert_eq!(classify("HY000", -1102, "The file is already in use."), ErrKind::Locked);
        assert_eq!(classify("HY000", -3035, "Operation must use an updateable query."), ErrKind::ReadOnly);
        assert_eq!(classify("23000", -1605, "The changes you requested to the table were not successful because they would create duplicate values"), ErrKind::Constraint);
        assert_eq!(classify("HY000", -1206, "Unrecognized database format 'x'."), ErrKind::Corrupt);
    }

    #[test]
    fn connection_string_quotes_only_when_needed() {
        assert_eq!(access::connection_string(Path::new(r"\\srv\a b\x.accdb"), false), format!("Driver={{{ACCESS_DRIVER}}};DBQ=\\\\srv\\a b\\x.accdb;"));
        assert!(access::connection_string(Path::new(r"C:\a;b\x.accdb"), true).contains("DBQ={C:\\a;b\\x.accdb};ReadOnly=1;"));
    }

    /// Exclusive lock, read-only file and maintenance mode against the real Access driver (test file only):
    ///   set ASR_ACCESS_TEST_DIR=C:\some\local\folder && cargo test --no-default-features -- --ignored access_ --nocapture
    #[test]
    #[ignore]
    fn access_locks_read_only_and_maintenance() {
        let Ok(dir) = std::env::var("ASR_ACCESS_TEST_DIR") else { return };
        let p = std::path::PathBuf::from(dir).join(format!("asr-lock-{}.accdb", uuid::Uuid::new_v4()));
        create_database(&p, "Access lock test", "test", "cargo test").unwrap();
        // another program holds the file exclusively
        let holder = AccessConn::open_with(&p, false, "Exclusive=1;").unwrap();
        let r = probe(&p, &device("A"));
        println!("exclusive lock -> {:?}", r.error.as_ref().map(|e| (e.kind, e.provider_detail(), e.message.clone())));
        assert!(!r.writable);
        assert_eq!(r.error.unwrap().kind, ErrKind::Locked);
        drop(holder);
        assert!(probe(&p, &device("A")).writable, "lock released -> writable again");
        // read-only file
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&p, perm.clone()).unwrap();
        let r = probe(&p, &device("B"));
        perm.set_readonly(false);
        std::fs::set_permissions(&p, perm).unwrap();
        println!("read-only file -> readable {} writable {} {:?}", r.readable, r.writable, r.error.as_ref().map(|e| (e.kind, e.provider_detail())));
        assert!(r.readable && !r.writable);
        assert_eq!(r.error.unwrap().kind, ErrKind::ReadOnly);
        // maintenance blocks operations; upgrade on a current schema is a no-op that keeps data
        let mut c = open(&p, false).unwrap();
        set_meta(c.as_mut(), "maintenance", "1").unwrap();
        let op = crate::shared_apply::Op { op_id: "op-maint".into(), op_type: "staff.upsert".into(), entity_id: Some("s1".into()), proposed: serde_json::json!({"display_name": "Aaron", "active": true}), ..Default::default() };
        assert_eq!(crate::shared_apply::apply(c.as_mut(), &op).unwrap_err().kind, ErrKind::Maintenance);
        set_meta(c.as_mut(), "maintenance", "0").unwrap();
        assert_eq!(crate::shared_apply::apply(c.as_mut(), &op).unwrap().result, "accepted");
        assert_eq!(upgrade_schema(c.as_mut(), "admin").unwrap().schema_version, SHARED_SCHEMA_VERSION);
        assert_eq!(col_i(&c.query("SELECT COUNT(*) FROM Staff", &[]).unwrap()[0], 0), 1);
    }

    /// Real Access driver check against a NEW test database (never the production path):
    ///   set ASR_ACCESS_TEST_DIR=C:\some\local\folder && cargo test --no-default-features -- --ignored access_ --nocapture
    #[test]
    #[ignore]
    fn access_create_probe_and_reject_existing() {
        let Ok(dir) = std::env::var("ASR_ACCESS_TEST_DIR") else { return };
        let p = std::path::PathBuf::from(dir).join(format!("asr-test-{}.accdb", uuid::Uuid::new_v4()));
        let id = create_database(&p, "Access integration test", "test", "cargo test").unwrap();
        println!("created {} id {}", p.display(), id.database_id);
        let r = probe(&p, &device("A"));
        println!("probe: reachable {} readable {} writable {} driver {:?} {:?} in {} ms", r.reachable, r.readable, r.writable, r.driver, r.error, r.elapsed_ms);
        assert!(r.readable && r.writable);
        assert_eq!(r.identity.unwrap().database_id, id.database_id);
        assert!(create_database(&p, "x", "test", "t").is_err());
        let r = probe(&p.with_file_name("does-not-exist.accdb"), &device("A"));
        assert_eq!(r.error.unwrap().kind, ErrKind::FileNotFound);
    }
}
