//! Where things live on disk. Everything the app writes sits under one folder, so removing it
//! removes the app's environments, clips and history (model weights stay in the shared Hugging
//! Face cache, where other tools can reuse them).

use std::path::{Path, PathBuf};

use crate::catalog::CATALOG_JSON;

const WORKER_PY: &str = include_str!("../sidecar/worker.py");
const SETUP_PY: &str = include_str!("../sidecar/setup.py");
const SOPHON_PY: &str = include_str!("../sidecar/sophon.py");
const TOOLS_PY: &str = include_str!("../sidecar/tools.py");

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into()))
}

/// `~/Library/Application Support/Ventriloquist`, or `$VENTRILOQUIST_HOME`.
pub fn data_dir() -> PathBuf {
    match std::env::var_os("VENTRILOQUIST_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join("Library/Application Support/Ventriloquist"),
    }
}

pub fn sidecar_dir() -> PathBuf {
    data_dir().join("sidecar")
}

pub fn env_python(env: &str) -> PathBuf {
    data_dir().join("envs").join(env).join("bin/python")
}

/// The installer writes this once a model's environment is built and its weights are on disk.
pub fn installed_marker(model: &str) -> PathBuf {
    data_dir().join("installed").join(format!("{model}.json"))
}

pub fn takes_dir() -> PathBuf {
    data_dir().join("takes")
}

pub fn clips_dir() -> PathBuf {
    data_dir().join("clips")
}

/// Where a voice's preview is kept once it has been made, so the next listen is instant.
pub fn preview_path(model: &str, voice: &str) -> PathBuf {
    let name: String = voice.chars().map(|c| if c == '/' || c == ':' { '_' } else { c }).collect();
    data_dir().join("previews").join(model).join(format!("{name}.wav"))
}

pub fn settings_file() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn history_file() -> PathBuf {
    data_dir().join("history.json")
}

/// Writes the embedded worker, installer and catalog out, so the app is one file to ship and
/// the Python side always matches the binary that drives it.
pub fn install_sidecar() -> std::io::Result<()> {
    let dir = sidecar_dir();
    std::fs::create_dir_all(&dir)?;
    for sub in ["takes", "clips", "installed", "envs"] {
        std::fs::create_dir_all(data_dir().join(sub))?;
    }
    for (name, body) in [("worker.py", WORKER_PY), ("setup.py", SETUP_PY), ("sophon.py", SOPHON_PY), ("tools.py", TOOLS_PY), ("catalog.json", CATALOG_JSON)] {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).is_ok_and(|old| old == body) {
            continue;
        }
        std::fs::write(path, body)?;
    }
    Ok(())
}

/// A GUI app starts with a bare `PATH`, so tools a shell would find are looked up by hand.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let home = home();
    let fixed = [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".local/bin"),
        home.join(".cargo/bin"),
    ];
    let from_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    fixed.iter().chain(from_path.iter()).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// `PATH` for child processes: the usual tool folders first, then whatever the app was given.
pub fn child_path() -> std::ffi::OsString {
    let mut dirs = vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin"), home().join(".local/bin")];
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// A file name people can read: `kokoro-0007.wav`.
pub fn take_path(model: &str, id: u64) -> PathBuf {
    takes_dir().join(format!("{model}-{id:04}.wav"))
}

pub fn display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}
