//! Shared fixtures for the pre-push coverage tests (v0.6 spec AC 277a).
//!
//! Every scenario is a real git repository in a temp dir with one or more
//! bare repositories as its remotes, driven with the `git` CLI so that
//! `git ls-remote` and `git push` behave as they do for a user. Git runs
//! hermetically: no system config, and a global config file of our own.
//!
//! Lippa is a `mockito` server that records each session-create body (the
//! bundle prompt), so a test can assert exactly which content reached the
//! bundle. A prompt containing [`DEFECT`] gets a high-severity finding
//! (exit 1); any other prompt converges clean (exit 0).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

pub const ZERO: &str = "0000000000000000000000000000000000000000";

/// The marker a mock reviewer treats as a blocking defect.
pub const DEFECT: &str = "QUORUM_TEST_DEFECT_MARKER";

const SESSIONS: &str = "/api/v1/consensus/sessions";

/// A mock Lippa that records every bundle prompt it receives.
pub struct Lippa {
    server: mockito::ServerGuard,
    prompts: Arc<Mutex<Vec<String>>>,
    _mocks: Vec<mockito::Mock>,
}

impl Lippa {
    pub fn new() -> Self {
        let mut server = mockito::Server::new();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&prompts);
        let create = server
            .mock("POST", SESSIONS)
            .with_status(201)
            .with_body_from_request(move |req| {
                let body = String::from_utf8_lossy(req.body().map(|b| b.as_slice()).unwrap_or(&[]))
                    .into_owned();
                let sess = if body.contains(DEFECT) {
                    "sess_high"
                } else {
                    "sess_clean"
                };
                seen.lock().unwrap().push(body);
                format!(
                    r#"{{"id":"{sess}","status":"pending","credits_reserved":3,"credits_remaining":197}}"#
                )
                .into_bytes()
            })
            .create();
        let mut mocks = vec![create];
        for (sess, divergence) in [
            (
                "sess_high",
                r#"[{"claim_text":"Blocking defect introduced","confidence":0.9,"supported_by":["m1"]}]"#,
            ),
            ("sess_clean", "[]"),
        ] {
            mocks.push(
                server
                    .mock("GET", format!("{SESSIONS}/{sess}/status").as_str())
                    .with_status(200)
                    .with_body(r#"{"status":"converged","rounds_completed":3,"rounds_total":3,"models_active":3,"models_dropped":0,"elapsed_seconds":4.0,"follow_up_questions":null}"#)
                    .create(),
            );
            mocks.push(
                server
                    .mock("GET", format!("{SESSIONS}/{sess}").as_str())
                    .with_status(200)
                    .with_body(format!(
                        r#"{{"id":"{sess}","status":"converged","summary_text":"x","agreement":[],"divergence":{divergence},"assumptions":[],"models":[]}}"#
                    ))
                    .create(),
            );
        }
        Lippa {
            server,
            prompts,
            _mocks: mocks,
        }
    }

    pub fn url(&self) -> String {
        self.server.url()
    }

    /// Every bundle prompt received so far, in order.
    pub fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }
}

/// A working repository plus the bare repositories it pushes to.
pub struct Fixture {
    root: TempDir,
    /// The repository under test (where `quorum` runs).
    pub work: PathBuf,
    gitconfig: PathBuf,
}

impl Fixture {
    /// An empty working repository on `main`, with no remotes.
    pub fn new() -> Self {
        let root = TempDir::new().unwrap();
        let gitconfig = root.path().join("gitconfig");
        std::fs::write(
            &gitconfig,
            "[user]\n\tname = t\n\temail = t@e\n[init]\n\tdefaultBranch = main\n\
             [core]\n\tautocrlf = false\n[protocol \"file\"]\n\tallow = always\n",
        )
        .unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let fx = Fixture {
            root,
            work,
            gitconfig,
        };
        fx.git(&["init", "-q", "-b", "main"]);
        fx
    }

    /// A path inside the fixture's temp dir.
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    /// A new bare repository `<name>.git` in the temp dir, added to the
    /// working repository as remote `remote`. Returns its path.
    pub fn add_bare_remote(&self, remote: &str, name: &str) -> PathBuf {
        let bare = self.path(&format!("{name}.git"));
        self.git_in(
            self.root.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        self.git(&["remote", "add", remote, bare.to_str().unwrap()]);
        bare
    }

    /// Apply the hermetic git environment to `cmd`.
    pub fn git_env(&self, cmd: &mut Command) {
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_TERMINAL_PROMPT", "0");
    }

    /// Run `git <args>` in the working repository; panics on failure.
    /// Returns trimmed stdout.
    pub fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.work, args)
    }

    /// Run `git <args>` in `dir`; panics on failure.
    pub fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        self.git_env(&mut cmd);
        let out = cmd.output().expect("`git` must be on PATH");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Write `path` (relative to the working tree) and commit it alone.
    /// Returns the new commit's sha.
    pub fn commit_file(&self, path: &str, content: &str, msg: &str) -> String {
        let full = self.work.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, content).unwrap();
        self.git(&["add", "--", path]);
        self.git(&["commit", "-q", "-m", msg]);
        self.rev("HEAD")
    }

    /// Delete `path` and commit the deletion. Returns the new commit's sha.
    pub fn remove_file(&self, path: &str, msg: &str) -> String {
        self.git(&["rm", "-q", "--", path]);
        self.git(&["commit", "-q", "-m", msg]);
        self.rev("HEAD")
    }

    pub fn rev(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev])
    }

    /// Write `.quorum/config.toml` pointing at `lippa`, plus `extra`
    /// (e.g. a `[bundle]` section). Untracked, as in a real linked repo.
    pub fn link(&self, lippa: &Lippa, extra: &str) {
        let qd = self.work.join(".quorum");
        std::fs::create_dir_all(&qd).unwrap();
        std::fs::write(
            qd.join("config.toml"),
            format!(
                "project_id = \"p_1\"\nbase_url = \"{}\"\n{extra}",
                lippa.url()
            ),
        )
        .unwrap();
    }

    /// The environment every `quorum` run needs: a session for `lippa`,
    /// no keyring or login env, an isolated config dir, hermetic git.
    pub fn quorum_env(&self, cmd: &mut Command, lippa: &Lippa) {
        let cfg = self.path("quorum-config");
        std::fs::create_dir_all(&cfg).unwrap();
        cmd.env("APPDATA", &cfg)
            .env("XDG_CONFIG_HOME", &cfg)
            .env_remove("QUORUM_LIPPA_EMAIL")
            .env_remove("QUORUM_LIPPA_PASSWORD")
            .env_remove("QUORUM_HOOK_POLICY")
            .env_remove("QUORUM_SKIP")
            .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
            .env("QUORUM_LIPPA_BASE_URL", lippa.url());
        self.git_env(cmd);
    }

    /// Run `quorum review --hook-mode=pre-push <args>` with `stdin`, the
    /// way a hook would. Returns the exit code and stderr.
    pub fn pre_push(&self, lippa: &Lippa, args: &[&str], stdin: &str) -> (Option<i32>, String) {
        let mut cmd = Command::new(quorum_bin());
        cmd.current_dir(&self.work)
            .arg("review")
            .arg("--hook-mode=pre-push")
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        self.quorum_env(&mut cmd, lippa);
        let mut child = cmd.spawn().unwrap();
        {
            use std::io::Write;
            let mut sin = child.stdin.take().unwrap();
            sin.write_all(stdin.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        println!("exit {:?}\n{stderr}", out.status.code());
        (out.status.code(), stderr)
    }

    /// Run a real `git push <args>` from the working repository, with the
    /// test's `quorum` binary first on `PATH` so an installed hook runs it.
    /// Returns whether the push succeeded, and its stderr.
    pub fn git_push(&self, lippa: &Lippa, args: &[&str]) -> (bool, String) {
        let bin_dir = quorum_bin().parent().unwrap().to_path_buf();
        let mut path = vec![bin_dir];
        path.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.work)
            .arg("push")
            .args(args)
            .env("PATH", std::env::join_paths(path).unwrap());
        self.quorum_env(&mut cmd, lippa);
        let out = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        println!("git push {args:?}: {:?}\n{stderr}", out.status.code());
        (out.status.success(), stderr)
    }
}

/// The `quorum` binary under test.
pub fn quorum_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("quorum")
}

/// One pre-push stdin line for a new branch (`remote_sha` all zero).
pub fn new_branch_tuple(branch: &str, local_sha: &str) -> String {
    format!("refs/heads/{branch} {local_sha} refs/heads/{branch} {ZERO}\n")
}

/// One pre-push stdin line updating an existing remote branch.
pub fn update_tuple(branch: &str, local_sha: &str, remote_sha: &str) -> String {
    format!("refs/heads/{branch} {local_sha} refs/heads/{branch} {remote_sha}\n")
}
