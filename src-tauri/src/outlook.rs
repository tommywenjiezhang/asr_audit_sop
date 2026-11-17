//! Bridge to the short-lived Python Outlook COM worker.
//! One process per operation: JSON request on stdin, one JSON response on stdout.
//! No live COM objects ever cross the process boundary.

use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkerResponse {
    pub ok: bool,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub diagnostics: Value,
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Interpreter (python.exe / py.exe) or a packaged worker .exe.
    pub program: PathBuf,
    /// Extra args before the script (e.g. ["-3"] for py.exe, [script] for python).
    pub args: Vec<String>,
}

impl WorkerConfig {
    /// Resolve how to run the worker: packaged exe > configured python > py launcher > python on PATH.
    /// `resource_dir` is the folder of the app executable (installed or extracted ZIP, any path).
    /// `dev_dir` is the source checkout and is only passed by development (debug) builds.
    pub fn resolve(python_path: &str, resource_dir: Option<&Path>, dev_dir: Option<&Path>) -> AppResult<WorkerConfig> {
        let candidates_dirs: Vec<PathBuf> = resource_dir
            .map(|r| vec![r.join("outlook_worker"), r.to_path_buf()])
            .unwrap_or_default()
            .into_iter()
            .chain(dev_dir.map(Path::to_path_buf))
            .collect();
        for d in &candidates_dirs {
            let exe = d.join("outlook_worker.exe");
            if exe.is_file() {
                return Ok(WorkerConfig { program: exe, args: vec![] });
            }
        }
        let script = candidates_dirs
            .iter()
            .map(|d| d.join("outlook_worker.py"))
            .find(|p| p.is_file())
            .ok_or_else(|| AppError::coded("WORKER_MISSING", "Outlook worker script (outlook_worker.py) was not found."))?;
        let script_s = script.to_string_lossy().to_string();
        // -I: isolated mode (ignores user site/env vars, does not add CWD to sys.path)
        if !python_path.trim().is_empty() {
            let p = PathBuf::from(python_path.trim());
            let is_py_launcher = p.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("py.exe")).unwrap_or(false);
            let mut args = if is_py_launcher { vec!["-3".to_string()] } else { vec![] };
            args.extend(["-I".to_string(), script_s]);
            return Ok(WorkerConfig { program: p, args });
        }
        if let Some(py) = which("py.exe") {
            return Ok(WorkerConfig { program: py, args: vec!["-3".into(), "-I".into(), script_s] });
        }
        if let Some(py) = which("python.exe") {
            return Ok(WorkerConfig { program: py, args: vec!["-I".into(), script_s] });
        }
        Err(AppError::coded("PYTHON_MISSING", "Python was not found. Install Python 3 with pywin32, or set the Python path in Settings."))
    }
}

fn which(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(exe))
        // skip the Microsoft Store alias stub, which opens the Store instead of running Python
        .find(|p| p.is_file() && !p.to_string_lossy().to_lowercase().contains("windowsapps"))
}

/// Run one worker operation with a timeout.
pub async fn call(cfg: &WorkerConfig, op: &str, params: Value, timeout: Duration) -> AppResult<WorkerResponse> {
    let started = std::time::Instant::now();
    let mut cmd = tokio::process::Command::new(&cfg.program);
    cmd.args(&cfg.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::coded("WORKER_START_FAILED", format!("Could not start the Outlook worker ({}): {e}", cfg.program.display())))?;
    let req = serde_json::json!({ "op": op, "params": params });
    {
        let mut stdin = child.stdin.take().ok_or_else(|| AppError::msg("worker stdin unavailable"))?;
        stdin.write_all(serde_json::to_string(&req)?.as_bytes()).await?;
        stdin.shutdown().await?;
    }
    let mut stdout = child.stdout.take().ok_or_else(|| AppError::msg("worker stdout unavailable"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| AppError::msg("worker stderr unavailable"))?;
    let run = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (r1, r2) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        r1?;
        r2?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, out, err))
    };
    let (status, out, err) = match tokio::time::timeout(timeout, run).await {
        Ok(r) => r?,
        Err(_) => {
            return Err(AppError::coded("WORKER_TIMEOUT", format!("Outlook did not respond within {} seconds.", timeout.as_secs())));
        }
    };
    let elapsed = started.elapsed().as_millis();
    let text = String::from_utf8_lossy(&out);
    // the response is the last non-empty line (defensive against stray prints)
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
    match serde_json::from_str::<WorkerResponse>(line) {
        Ok(resp) => {
            tracing::info!(op, ok = resp.ok, code = ?resp.error_code, elapsed_ms = elapsed as u64, "outlook worker finished");
            if !resp.ok {
                tracing::warn!(op, diagnostics = %resp.diagnostics, "outlook worker reported an error");
            }
            Ok(resp)
        }
        Err(_) => {
            let stderr_tail: String = String::from_utf8_lossy(&err).chars().rev().take(800).collect::<String>().chars().rev().collect();
            tracing::error!(op, exit = ?status.code(), stderr = %stderr_tail, "outlook worker returned no valid JSON");
            Err(AppError::coded("WORKER_BAD_OUTPUT", format!("The Outlook worker failed (exit code {:?}). See the log for details.", status.code())))
        }
    }
}

/// Convert a failed worker response into an AppError.
pub fn into_result(resp: WorkerResponse) -> AppResult<Value> {
    if resp.ok {
        Ok(resp.data)
    } else {
        Err(AppError::coded(
            resp.error_code.unwrap_or_else(|| "OUTLOOK_ERROR".into()),
            resp.message.unwrap_or_else(|| "Outlook operation failed".into()),
        ))
    }
}

/// Error codes that are worth retrying (Outlook busy / transient COM failures).
pub fn is_transient(code: &str) -> bool {
    matches!(code, "COM_ERROR" | "OUTLOOK_BUSY" | "WORKER_TIMEOUT" | "OUTLOOK_UNAVAILABLE")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end bridge test against the real worker script using its built-in mock mode.
    #[tokio::test]
    async fn bridge_runs_worker_in_mock_mode() {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../outlook_worker");
        let Ok(cfg) = WorkerConfig::resolve("", None, Some(&dev)) else {
            eprintln!("python not available; skipping");
            return;
        };
        let resp = call(&cfg, "test_connection", serde_json::json!({"mock": true}), Duration::from_secs(60)).await.unwrap();
        assert!(resp.ok, "{resp:?}");
        let resp = call(&cfg, "no_such_op", serde_json::json!({"mock": true}), Duration::from_secs(60)).await.unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error_code.as_deref(), Some("BAD_REQUEST"));
    }

    /// The packaged layout `<app folder>\outlook_worker\outlook_worker.exe` is found from the app folder
    /// alone, also when the folder path contains spaces; nothing else is consulted.
    #[test]
    fn packaged_worker_found_next_to_app_in_folder_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("My Apps").join("ASR Desk 1.0");
        std::fs::create_dir_all(app.join("outlook_worker")).unwrap();
        let exe = app.join("outlook_worker").join("outlook_worker.exe");
        std::fs::write(&exe, b"stub").unwrap();
        let cfg = WorkerConfig::resolve("", Some(&app), None).unwrap();
        assert_eq!(cfg.program, exe);
        assert!(cfg.args.is_empty(), "the packaged worker needs no Python");
    }
}
