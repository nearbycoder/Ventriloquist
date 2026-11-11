"""Installs models for Ventriloquist: builds each model's Python environment with uv and
downloads its weights into the shared Hugging Face cache.

    uv run --no-project --python 3.12 setup.py --data-dir DIR MODEL [MODEL ...]
    uv run --no-project --python 3.12 setup.py --data-dir DIR --all
    uv run --no-project --python 3.12 setup.py --data-dir DIR --remove MODEL [MODEL ...]
    uv run --no-project --python 3.12 setup.py --list

Standard library only, so it runs before any environment exists. The app runs it for its
Install buttons; `scripts/setup-models` runs it from a terminal. Safe to run again: finished
steps are skipped.
"""

import argparse
import fcntl
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
CATALOG = json.loads((HERE / "catalog.json").read_text())
MODELS = {m["id"]: m for m in CATALOG["models"]}

DOWNLOAD = """
import sys, json
from huggingface_hub import snapshot_download
spec = json.loads(sys.argv[1])
path = snapshot_download(spec["repo"], allow_patterns=spec.get("allow"), ignore_patterns=spec.get("ignore"))
print("  in", path)
"""


# Deletes cached repositories through the library that wrote them, which knows the cache's
# layout and leaves alone any file another repository still uses.
DELETE = """
import sys, json
from huggingface_hub import scan_cache_dir
wanted = set(json.loads(sys.argv[1]))
cache = scan_cache_dir()
revisions = [rev.commit_hash for repo in cache.repos if repo.repo_type == "model" and repo.repo_id in wanted for rev in repo.revisions]
if revisions:
    plan = cache.delete_revisions(*revisions)
    print("  freeing", plan.expected_freed_size_str)
    plan.execute()
"""


def say(*parts):
    print(*parts, flush=True)


def find_uv():
    found = shutil.which("uv")
    if found:
        return found
    for candidate in ("/opt/homebrew/bin/uv", "/usr/local/bin/uv", "~/.local/bin/uv", "~/.cargo/bin/uv"):
        path = Path(candidate).expanduser()
        if path.is_file():
            return str(path)
    sys.exit("uv isn't installed. Install it with `brew install uv`, then try again.")


def run(cmd, **kw):
    """Runs a command, passing its output through, and stops the install if it fails."""
    result = subprocess.run([str(c) for c in cmd], **kw)
    if result.returncode != 0:
        sys.exit(f"Failed ({result.returncode}): {' '.join(str(c) for c in cmd[:6])} ...")


def default_data_dir():
    override = os.environ.get("VENTRILOQUIST_HOME")
    return Path(override) if override else Path.home() / "Library/Application Support/Ventriloquist"


def ensure_env(uv, data, name):
    """Creates the environment and installs its packages, once per change to its package list."""
    spec = CATALOG["envs"][name]
    env = data / "envs" / name
    python = env / "bin" / "python"
    stamp = env / ".ventriloquist-packages"
    want = hashlib.sha256(json.dumps(spec, sort_keys=True).encode()).hexdigest()
    (data / "envs").mkdir(parents=True, exist_ok=True)
    # Two installs that share an environment take turns building it.
    with open(data / "envs" / f".{name}.lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if python.is_file() and stamp.is_file() and stamp.read_text() == want:
            say(f"Environment {name}: up to date")
            return python
        missing = [tool for tool in spec.get("system", []) if not shutil.which(tool)]
        if missing:
            sys.exit(f"The {name} environment needs {', '.join(missing)}. Install with: brew install {' '.join(missing)}")
        if not python.is_file():
            say(f"Environment {name}: creating with Python {spec['python']}")
            run([uv, "venv", "--python", spec["python"], env])
        if spec["packages"]:
            say(f"Environment {name}: installing {len(spec['packages'])} packages")
            cmd = [uv, "pip", "install", "--python", python]
            for extra in spec.get("pip_args", []):
                cmd.append(extra)
            run(cmd + spec["packages"])
        # Models that ship as a repository rather than a package are checked out at a fixed
        # commit inside the environment.
        for repo in spec.get("git", []):
            dest = env / "src" / repo["dir"]
            if not (dest / ".git").is_dir():
                say(f"Environment {name}: cloning {repo['url']}")
                dest.parent.mkdir(parents=True, exist_ok=True)
                run(["git", "clone", "--quiet", repo["url"], dest])
            run(["git", "-C", dest, "checkout", "--quiet", "--detach", repo["rev"]])
            for sub in repo.get("submodules", []):
                run(["git", "-C", dest, "submodule", "update", "--quiet", "--init", "--depth", "1", sub])
            # A repository with a lockfile of its own installs from it, into this environment:
            # the versions its authors ran, not whatever resolves today.
            if repo.get("sync"):
                say(f"Environment {name}: installing {repo['sync']} from its lockfile")
                run([uv, "sync", "--quiet", "--locked", "--package", repo["sync"], "--project", dest], env={**os.environ, "UV_PROJECT_ENVIRONMENT": str(env)})
        stamp.write_text(want)
        return python


def install(uv, data, model_id):
    model = MODELS[model_id]
    started = time.time()
    say(f"Installing {model['name']}")
    python = ensure_env(uv, data, model["env"])
    for spec in model.get("downloads", []):
        say(f"Downloading {spec['repo']}")
        run([python, "-c", DOWNLOAD, json.dumps(spec)])
    say(f"Loading {model['name']} once to check it")
    run([python, HERE / "worker.py", "--prepare", model_id], env={**os.environ, "TOKENIZERS_PARALLELISM": "false", "PYTORCH_ENABLE_MPS_FALLBACK": "1"})
    marker = data / "installed" / f"{model_id}.json"
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text(json.dumps({"model": model_id, "installed": int(time.time())}))
    say(f"{model['name']} is ready ({time.time() - started:.0f} s)")


def installed_models(data):
    return [m for m in MODELS if (data / "installed" / f"{m}.json").is_file()]


def remove(data, model_id):
    """Takes a model off this Mac: its weights, and its environment once nothing else uses it.

    Weights another installed model shares (the transcriber two cloning models use) stay.
    """
    model = MODELS[model_id]
    say(f"Removing {model['name']}")
    python = data / "envs" / model["env"] / "bin" / "python"
    # The marker goes first: from here on the app no longer offers the model.
    (data / "installed" / f"{model_id}.json").unlink(missing_ok=True)
    remaining = installed_models(data)
    shared = {spec["repo"] for other in remaining for spec in MODELS[other].get("downloads", [])}
    # A repository marked `keep` belongs to other apps too (Figment uses the enhancer's model),
    # so removing the model here leaves its weights in the shared cache.
    repos = [spec["repo"] for spec in model.get("downloads", []) if spec["repo"] not in shared and not spec.get("keep")]
    if repos and python.is_file():
        say(f"Deleting {', '.join(repos)}")
        run([python, "-c", DELETE, json.dumps(repos)])
    # A few models keep weights outside the Hugging Face cache. Only folders the app's own
    # libraries made are ever deleted: inside the data folder, or coqui-tts's model store.
    allowed = [data.resolve(), (Path.home() / "Library/Application Support/tts").resolve()]
    for entry in model.get("files", []):
        path = Path(entry.replace("{data}", str(data))).expanduser().resolve()
        if path.is_dir() and any(root in path.parents for root in allowed):
            say(f"Deleting {path}")
            shutil.rmtree(path)
    previews = data / "previews" / model_id
    if previews.is_dir():
        shutil.rmtree(previews)
    if not any(MODELS[other]["env"] == model["env"] for other in remaining):
        env = data / "envs" / model["env"]
        with open(data / "envs" / f".{model['env']}.lock", "w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            if env.is_dir():
                say(f"Deleting the {model['env']} environment")
                shutil.rmtree(env)
    say(f"{model['name']} is removed")


def main():
    parser = argparse.ArgumentParser(description="Install Ventriloquist models.")
    parser.add_argument("models", nargs="*", help="model ids; see --list")
    parser.add_argument("--all", action="store_true", help="install every model in the catalog")
    parser.add_argument("--remove", action="store_true", help="remove the named models instead of installing them")
    parser.add_argument("--list", action="store_true", help="list the models and exit")
    parser.add_argument("--data-dir", type=Path, default=None, help="where environments live (default: the app's folder)")
    args = parser.parse_args()
    data = args.data_dir or default_data_dir()
    if args.list:
        for m in CATALOG["models"]:
            done = "installed" if (data / "installed" / f"{m['id']}.json").is_file() else "-"
            say(f"{m['id']:<22} {m['kind']:<5} {m['size']:>8}  {done:<10} {m['name']}")
        return
    wanted = list(MODELS) if args.all else args.models
    unknown = [m for m in wanted if m not in MODELS]
    if unknown or not wanted:
        sys.exit(f"Unknown model: {', '.join(unknown)}" if unknown else "Name a model, or pass --all. See --list.")
    if args.remove:
        if args.all:
            sys.exit("Name the models to remove.")
        for model_id in wanted:
            remove(data, model_id)
        return
    uv = find_uv()
    for model_id in wanted:
        install(uv, data, model_id)


if __name__ == "__main__":
    main()
