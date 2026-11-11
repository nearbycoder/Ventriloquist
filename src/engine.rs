//! The Python side: one long-lived worker process per environment, which keeps its models
//! loaded between runs, and one-shot installer processes. Both talk over pipes; reader threads
//! turn their lines into `Msg`s and wake the window, and `poll` drains them on the UI thread.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use rune::Waker;
use serde_json::Value;

use crate::paths;

pub enum Msg {
    /// A line of the worker's protocol: `{"event": "status" | "done" | "error", "id": .., ..}`.
    Worker { value: Value },
    /// Something a library printed. Kept for the error a failed run shows.
    WorkerLog { env: String, line: String },
    WorkerExit { env: String, generation: u64 },
    Setup { model: String, line: String },
    SetupExit { model: String, ok: bool },
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    generation: u64,
}

type SharedWaker = Arc<Mutex<Option<Waker>>>;

pub struct Engine {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    waker: SharedWaker,
    workers: HashMap<String, Worker>,
    /// Running installers' process ids, by model.
    installers: HashMap<String, u32>,
    generation: u64,
}

fn wake(waker: &SharedWaker) {
    let waker = waker.lock().ok().and_then(|w| w.clone());
    if let Some(w) = waker {
        w();
    }
}

/// Feeds a pipe's lines to `f` until it closes. A carriage return ends a line too, so a
/// download's progress bar arrives as it moves instead of when it finishes.
fn pump(reader: impl std::io::Read + Send + 'static, mut f: impl FnMut(String) + Send + 'static) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&buf);
            if let Some(line) = text.rsplit('\r').map(str::trim).find(|l| !l.is_empty()) {
                f(line.to_owned());
            }
        }
    });
}

impl Engine {
    pub fn new() -> Engine {
        let (tx, rx) = channel();
        Engine { tx, rx, waker: Arc::default(), workers: HashMap::new(), installers: HashMap::new(), generation: 0 }
    }

    pub fn set_waker(&self, waker: Waker) {
        *self.waker.lock().unwrap() = Some(waker);
    }

    pub fn drain(&self) -> Vec<Msg> {
        self.rx.try_iter().collect()
    }

    pub fn is_running(&self, env: &str) -> bool {
        self.workers.contains_key(env)
    }

    pub fn running(&self) -> usize {
        self.workers.len()
    }

    /// Whether the exit a reader thread reported is that of the worker now running in `env`,
    /// not one that was stopped and replaced.
    pub fn exited(&mut self, env: &str, generation: u64) -> bool {
        if self.workers.get(env).is_some_and(|w| w.generation == generation) {
            if let Some(mut w) = self.workers.remove(env) {
                w.child.wait().ok();
            }
            return true;
        }
        false
    }

    fn spawn(&mut self, env: &str) -> Result<(), String> {
        let python = paths::env_python(env);
        if !python.is_file() {
            return Err(format!("The {env} environment isn't installed."));
        }
        let mut child = Command::new(&python)
            .arg("-u")
            .arg(paths::sidecar_dir().join("worker.py"))
            .current_dir(paths::data_dir())
            .env("PATH", paths::child_path())
            .env("PYTHONUNBUFFERED", "1")
            .env("TOKENIZERS_PARALLELISM", "false")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            // Ops a model needs that Metal lacks run on the CPU instead of failing.
            .env("PYTORCH_ENABLE_MPS_FALLBACK", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Couldn't start the {env} worker: {e}"))?;
        self.generation += 1;
        let generation = self.generation;
        let stdin = child.stdin.take().expect("piped");
        let (stdout, stderr) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));

        let (tx, waker, name) = (self.tx.clone(), self.waker.clone(), env.to_owned());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let msg = match serde_json::from_str::<Value>(&line) {
                    Ok(value) if value.get("event").is_some() => Msg::Worker { value },
                    _ => Msg::WorkerLog { env: name.clone(), line },
                };
                if tx.send(msg).is_err() {
                    return;
                }
                wake(&waker);
            }
            tx.send(Msg::WorkerExit { env: name, generation }).ok();
            wake(&waker);
        });
        let (tx, waker, name) = (self.tx.clone(), self.waker.clone(), env.to_owned());
        pump(stderr, move |line| {
            if tx.send(Msg::WorkerLog { env: name.clone(), line }).is_ok() {
                wake(&waker);
            }
        });
        self.workers.insert(env.to_owned(), Worker { child, stdin, generation });
        Ok(())
    }

    /// Sends one request to the environment's worker, starting it if it isn't up.
    pub fn send(&mut self, env: &str, request: &Value) -> Result<(), String> {
        if !self.workers.contains_key(env) {
            self.spawn(env)?;
        }
        let worker = self.workers.get_mut(env).expect("spawned");
        let sent = writeln!(worker.stdin, "{request}").and_then(|_| worker.stdin.flush());
        if let Err(e) = sent {
            self.stop(env);
            return Err(format!("The {env} worker went away: {e}"));
        }
        Ok(())
    }

    /// Ends an environment's worker, which frees its models' memory and cancels its run.
    pub fn stop(&mut self, env: &str) {
        if let Some(mut w) = self.workers.remove(env) {
            w.child.kill().ok();
            w.child.wait().ok();
        }
    }

    pub fn stop_all(&mut self) {
        for (_, mut w) in self.workers.drain() {
            w.child.kill().ok();
            w.child.wait().ok();
        }
        for (_, pid) in self.installers.drain() {
            Command::new("/bin/kill").arg(pid.to_string()).stderr(Stdio::null()).status().ok();
        }
    }

    /// Runs the installer for one model, reporting each line it prints: it builds the model's
    /// environment and downloads its weights, or with `remove` deletes them again.
    pub fn setup(&mut self, model: &str, remove: bool) -> Result<(), String> {
        let uv = paths::find_tool("uv").ok_or("uv isn't installed. Install it with `brew install uv`, then try again.")?;
        let mut command = Command::new(uv);
        command.args(["run", "--no-project", "--python", "3.12"]).arg(paths::sidecar_dir().join("setup.py")).arg("--data-dir").arg(paths::data_dir());
        if remove {
            command.arg("--remove");
        }
        let mut child = command
            .arg(model)
            .current_dir(paths::data_dir())
            .env("PATH", paths::child_path())
            .env("PYTHONUNBUFFERED", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Couldn't start the installer: {e}"))?;
        let (stdout, stderr) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
        for pipe in [Box::new(stdout) as Box<dyn std::io::Read + Send>, Box::new(stderr)] {
            let (tx, waker, name) = (self.tx.clone(), self.waker.clone(), model.to_owned());
            pump(pipe, move |line| {
                if tx.send(Msg::Setup { model: name.clone(), line }).is_ok() {
                    wake(&waker);
                }
            });
        }
        self.installers.insert(model.to_owned(), child.id());
        // The watcher owns the child, so a finished run wakes the window.
        let (tx, waker, name) = (self.tx.clone(), self.waker.clone(), model.to_owned());
        std::thread::spawn(move || {
            let ok = child.wait().is_ok_and(|s| s.success());
            tx.send(Msg::SetupExit { model: name, ok }).ok();
            wake(&waker);
        });
        Ok(())
    }

    pub fn installer_done(&mut self, model: &str) {
        self.installers.remove(model);
    }

    /// Stops a download in progress. What it already fetched stays in the cache, so starting
    /// again picks up where it left off.
    pub fn cancel_setup(&mut self, model: &str) {
        if let Some(pid) = self.installers.remove(model) {
            Command::new("/bin/kill").arg(pid.to_string()).stderr(Stdio::null()).status().ok();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop_all();
    }
}
