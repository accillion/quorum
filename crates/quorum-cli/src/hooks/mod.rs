//! Git hook installer / uninstaller (Phase 1B Stage 3).
//!
//! Two operations:
//!   * [`install`] — write `.git/hooks/<name>` from one of the embedded
//!     [`templates`] constants. Refuses to overwrite a non-Quorum-managed
//!     hook (idempotency marker scan; spec §4.5.4). Overwrites an
//!     existing Quorum-managed hook only while it is an unedited shipped
//!     template of the same type (any version, so v1 upgrades); refuses
//!     one with local edits.
//!   * [`uninstall`] — remove the hook file only if it is an unedited
//!     Quorum template (same rule); idempotent when the hook is absent.
//!
//! Path resolution uses libgit2 explicitly: `repo.path()` is the literal
//! `.git/` directory (correct under worktrees and custom `--git-dir`);
//! hooks live at `repo.path().join("hooks").join(<name>)`. We never use
//! `path().parent()` inference (v0.1 bug; v0.2 fixed).

mod legacy;
pub mod templates;

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use git2::Repository;

use templates::{HOOK_MARKER, PRE_COMMIT_TEMPLATE, PRE_PUSH_TEMPLATE};

/// Which hook to install / inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    PreCommit,
    PrePush,
}

impl HookKind {
    pub fn file_name(self) -> &'static str {
        match self {
            HookKind::PreCommit => "pre-commit",
            HookKind::PrePush => "pre-push",
        }
    }

    pub fn template(self) -> &'static str {
        match self {
            HookKind::PreCommit => PRE_COMMIT_TEMPLATE,
            HookKind::PrePush => PRE_PUSH_TEMPLATE,
        }
    }

    /// Every template Quorum has shipped for this hook, current first.
    /// A managed hook whose content is none of these has local edits.
    fn shipped_templates(self) -> [&'static str; 2] {
        match self {
            HookKind::PreCommit => [PRE_COMMIT_TEMPLATE, legacy::PRE_COMMIT_V1],
            HookKind::PrePush => [PRE_PUSH_TEMPLATE, legacy::PRE_PUSH_V1],
        }
    }

    /// Map the CLI string (`"pre-commit"` / `"pre-push"`) into this enum.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pre-commit" => Some(HookKind::PreCommit),
            "pre-push" => Some(HookKind::PrePush),
            _ => None,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum HooksError {
    #[error("not inside a git repository: {0}")]
    NotARepo(PathBuf),
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("existing hook is not Quorum-managed; remove it manually first ({0})")]
    NotManaged(PathBuf),
    /// The hook carries the Quorum marker but is not byte-for-byte a
    /// template Quorum shipped (CRLF aside): someone edited it. Refused
    /// by both install and uninstall; there is no override.
    #[error("existing hook is Quorum-managed but has local edits; move it aside first ({0})")]
    LocallyModified(PathBuf),
}

/// Outcome of an install / uninstall — useful for stderr framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    Written(PathBuf),
    Overwritten(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UninstallOutcome {
    Removed(PathBuf),
    /// No file at the path. Idempotent success.
    AlreadyAbsent(PathBuf),
}

/// Resolve `.git/hooks/<name>` for the repo containing `start`.
pub fn hook_path(start: &Path, kind: HookKind) -> Result<PathBuf, HooksError> {
    let repo =
        Repository::discover(start).map_err(|_| HooksError::NotARepo(start.to_path_buf()))?;
    // Spec §4.5.1: `repo.path()` is the literal `.git/` dir under
    // worktrees, custom `--git-dir`, and bare repos. Never use
    // `path().parent()` inference.
    Ok(repo.path().join("hooks").join(kind.file_name()))
}

/// Install `<repo>/.git/hooks/<name>` for the given hook kind.
///
/// - If the file is absent → write the template, return `Written`.
/// - If the file exists, the marker is present in the first 5 lines,
///   and the content is a template Quorum shipped for this hook (any
///   version) → overwrite, return `Overwritten`. This is both the
///   idempotent re-install and the v1 → v2 upgrade.
/// - Marker present but the content differs → `LocallyModified`: the
///   user's edits are never overwritten.
/// - Otherwise → `NotManaged` error (the user must remove the existing
///   hook manually).
///
/// On Unix the file mode is set to `0o755` (spec §4.5.4). On Windows the
/// inheriting ACL of `.git/hooks/` applies — Git for Windows runs hooks
/// via `sh.exe` regardless of NTFS executable bit.
pub fn install(start: &Path, kind: HookKind) -> Result<InstallOutcome, HooksError> {
    let path = hook_path(start, kind)?;
    let outcome = if path.exists() {
        refuse_unless_unedited(&path, kind)?;
        InstallOutcome::Overwritten(path.clone())
    } else {
        // Hooks dir may not exist on a fresh `git init` — create it.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        InstallOutcome::Written(path.clone())
    };
    fs::write(&path, kind.template())?;
    set_executable_unix(&path);
    Ok(outcome)
}

/// Remove `<repo>/.git/hooks/<name>` ONLY if it is an unedited Quorum
/// template (same rule as [`install`]). Idempotent when the file is
/// absent.
pub fn uninstall(start: &Path, kind: HookKind) -> Result<UninstallOutcome, HooksError> {
    let path = hook_path(start, kind)?;
    if !path.exists() {
        return Ok(UninstallOutcome::AlreadyAbsent(path));
    }
    refuse_unless_unedited(&path, kind)?;
    fs::remove_file(&path)?;
    Ok(UninstallOutcome::Removed(path))
}

/// `Ok` only when the existing hook at `path` is exactly a template
/// Quorum shipped for `kind`, LF or CRLF. Otherwise `NotManaged` (no
/// marker) or `LocallyModified` (marker, but edited). The one gate for
/// both install and uninstall, so neither can destroy a user's edits.
fn refuse_unless_unedited(path: &Path, kind: HookKind) -> Result<(), HooksError> {
    if !is_managed(path)? {
        return Err(HooksError::NotManaged(path.to_path_buf()));
    }
    let unedited = String::from_utf8(fs::read(path)?).is_ok_and(|s| {
        let s = s.replace("\r\n", "\n");
        kind.shipped_templates().contains(&s.as_str())
    });
    if unedited {
        Ok(())
    } else {
        Err(HooksError::LocallyModified(path.to_path_buf()))
    }
}

/// True when the file at `path` contains the [`HOOK_MARKER`] substring
/// somewhere in its first 5 lines (spec §4.5.4 — fixed from v0.1's
/// brittle "first non-shebang line" rule).
pub fn is_managed(path: &Path) -> Result<bool, HooksError> {
    let mut buf = String::new();
    fs::File::open(path)?.take(8192).read_to_string(&mut buf)?;
    Ok(first_five_lines_contain_marker(&buf))
}

fn first_five_lines_contain_marker(s: &str) -> bool {
    s.lines().take(5).any(|line| line.contains(HOOK_MARKER))
}

#[cfg(unix)]
fn set_executable_unix(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_executable_unix(_path: &Path) {
    // Windows: inheriting ACL of `.git/hooks/` governs execution.
    // Git for Windows runs hooks via `sh.exe` regardless of NTFS bit.
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_repo() -> TempDir {
        let td = TempDir::new().unwrap();
        Repository::init(td.path()).unwrap();
        td
    }

    #[test]
    fn parse_round_trip() {
        assert_eq!(HookKind::parse("pre-commit"), Some(HookKind::PreCommit));
        assert_eq!(HookKind::parse("pre-push"), Some(HookKind::PrePush));
        assert_eq!(HookKind::parse("pre-merge-commit"), None);
        assert_eq!(HookKind::PreCommit.file_name(), "pre-commit");
        assert_eq!(HookKind::PrePush.file_name(), "pre-push");
    }

    #[test]
    fn hook_path_under_dot_git_hooks() {
        let td = init_repo();
        let p = hook_path(td.path(), HookKind::PreCommit).unwrap();
        // Skip drive-letter dependency by checking the trailing segments.
        let trail: Vec<_> = p.components().rev().take(3).collect();
        assert_eq!(trail[0].as_os_str(), "pre-commit");
        assert_eq!(trail[1].as_os_str(), "hooks");
        assert_eq!(trail[2].as_os_str(), ".git");
    }

    #[test]
    fn install_writes_template_and_marker_present() {
        let td = init_repo();
        let outcome = install(td.path(), HookKind::PreCommit).unwrap();
        match outcome {
            InstallOutcome::Written(_) => {}
            other => panic!("first install must be Written, got {other:?}"),
        }
        let path = hook_path(td.path(), HookKind::PreCommit).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.starts_with("#!/bin/sh"));
        assert!(first_five_lines_contain_marker(&body));
        assert!(is_managed(&path).unwrap());
    }

    #[test]
    fn install_is_idempotent_for_managed_hook() {
        let td = init_repo();
        install(td.path(), HookKind::PrePush).unwrap();
        let again = install(td.path(), HookKind::PrePush).unwrap();
        match again {
            InstallOutcome::Overwritten(_) => {}
            other => panic!("re-install must be Overwritten, got {other:?}"),
        }
    }

    #[test]
    fn install_refuses_non_managed_existing_hook() {
        let td = init_repo();
        let path = hook_path(td.path(), HookKind::PreCommit).unwrap();
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(
            &path,
            "#!/bin/sh\n# some other tool\necho 'foreign'\nexit 0\n",
        )
        .unwrap();
        let err = install(td.path(), HookKind::PreCommit).unwrap_err();
        assert!(matches!(err, HooksError::NotManaged(_)));
        // File must be untouched.
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("some other tool"));
    }

    #[test]
    fn uninstall_removes_managed_idempotent_on_absent() {
        let td = init_repo();
        // First call before install → AlreadyAbsent.
        let absent = uninstall(td.path(), HookKind::PreCommit).unwrap();
        assert!(matches!(absent, UninstallOutcome::AlreadyAbsent(_)));
        // Install then uninstall → Removed; then AlreadyAbsent.
        install(td.path(), HookKind::PreCommit).unwrap();
        let removed = uninstall(td.path(), HookKind::PreCommit).unwrap();
        assert!(matches!(removed, UninstallOutcome::Removed(_)));
        let again = uninstall(td.path(), HookKind::PreCommit).unwrap();
        assert!(matches!(again, UninstallOutcome::AlreadyAbsent(_)));
    }

    #[test]
    fn uninstall_refuses_non_managed() {
        let td = init_repo();
        let path = hook_path(td.path(), HookKind::PrePush).unwrap();
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(&path, "#!/bin/sh\n# not ours\nexit 0\n").unwrap();
        let err = uninstall(td.path(), HookKind::PrePush).unwrap_err();
        assert!(matches!(err, HooksError::NotManaged(_)));
    }

    #[test]
    fn outside_repo_errs_with_not_a_repo() {
        let td = TempDir::new().unwrap(); // NOT git-init'd
        let err = hook_path(td.path(), HookKind::PreCommit).unwrap_err();
        assert!(matches!(err, HooksError::NotARepo(_)));
        let err = install(td.path(), HookKind::PreCommit).unwrap_err();
        assert!(matches!(err, HooksError::NotARepo(_)));
        let err = uninstall(td.path(), HookKind::PreCommit).unwrap_err();
        assert!(matches!(err, HooksError::NotARepo(_)));
    }

    #[test]
    fn marker_scan_first_five_lines_only() {
        // Marker on line 6 must NOT be detected.
        let s =
            "#!/bin/sh\n# line 2\n# line 3\n# line 4\n# line 5\n# quorum-managed-hook v1\nexit 0\n";
        assert!(!first_five_lines_contain_marker(s));
        // Marker on line 2 → detected.
        let s = "#!/bin/sh\n# quorum-managed-hook v1 (pre-commit)\nexit 0\n";
        assert!(first_five_lines_contain_marker(s));
    }

    #[cfg(unix)]
    #[test]
    fn install_sets_0755_mode_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let td = init_repo();
        let outcome = install(td.path(), HookKind::PreCommit).unwrap();
        let path = match outcome {
            InstallOutcome::Written(p) | InstallOutcome::Overwritten(p) => p,
        };
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    const BOTH: [HookKind; 2] = [HookKind::PreCommit, HookKind::PrePush];

    fn v1(kind: HookKind) -> &'static str {
        match kind {
            HookKind::PreCommit => legacy::PRE_COMMIT_V1,
            HookKind::PrePush => legacy::PRE_PUSH_V1,
        }
    }

    fn write_hook(td: &TempDir, kind: HookKind, body: &str) -> PathBuf {
        let path = hook_path(td.path(), kind).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    /// An unedited v1 hook, LF or CRLF, is replaced by the current
    /// template: `quorum install` is the upgrade path.
    #[test]
    fn install_upgrades_an_unedited_v1_hook() {
        for kind in BOTH {
            for body in [v1(kind).to_string(), v1(kind).replace('\n', "\r\n")] {
                let td = init_repo();
                let path = write_hook(&td, kind, &body);
                let outcome = install(td.path(), kind).unwrap();
                assert!(matches!(outcome, InstallOutcome::Overwritten(_)));
                assert_eq!(fs::read_to_string(&path).unwrap(), kind.template());
            }
        }
    }

    /// An unedited v1 hook is removed by `quorum uninstall`.
    #[test]
    fn uninstall_removes_an_unedited_v1_hook() {
        for kind in BOTH {
            let td = init_repo();
            let path = write_hook(&td, kind, v1(kind));
            let outcome = uninstall(td.path(), kind).unwrap();
            assert!(matches!(outcome, UninstallOutcome::Removed(_)));
            assert!(!path.exists());
        }
    }

    /// A Quorum-managed hook that no longer matches any template Quorum
    /// shipped for it has local edits. Neither `install` nor `uninstall`
    /// touches it, whatever version it started from.
    #[test]
    fn install_and_uninstall_refuse_a_managed_hook_with_local_edits() {
        let mut wrong = Vec::new();
        for kind in BOTH {
            for (version, base) in [("v1", v1(kind)), ("current", kind.template())] {
                let edited = format!("{base}\n# my team: also lint\nnpm run lint || exit 1\n");
                for op in ["install", "uninstall"] {
                    let td = init_repo();
                    let path = write_hook(&td, kind, &edited);
                    let result = if op == "install" {
                        install(td.path(), kind).map(|_| ())
                    } else {
                        uninstall(td.path(), kind).map(|_| ())
                    };
                    let refused =
                        matches!(&result, Err(e) if e.to_string().contains("local edits"));
                    let intact = fs::read_to_string(&path).ok().as_deref() == Some(&edited);
                    if !refused || !intact {
                        wrong.push(format!(
                            "{kind:?} {version}, {op}: {result:?}, file intact: {intact}"
                        ));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
