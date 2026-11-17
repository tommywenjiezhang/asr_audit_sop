use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("File error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Data format error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{message}")]
    Coded { code: String, message: String },
    #[error("{0}")]
    Msg(String),
}

impl AppError {
    pub fn msg(m: impl Into<String>) -> Self {
        AppError::Msg(m.into())
    }
    pub fn coded(code: impl Into<String>, message: impl Into<String>) -> Self {
        AppError::Coded { code: code.into(), message: message.into() }
    }
    pub fn code(&self) -> &str {
        match self {
            AppError::Db(_) => "DB_ERROR",
            AppError::Io(_) => "IO_ERROR",
            AppError::Json(_) => "JSON_ERROR",
            AppError::Coded { code, .. } => code,
            AppError::Msg(_) => "ERROR",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("AppError", 2)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;
