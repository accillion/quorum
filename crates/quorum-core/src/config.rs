//! `.quorum/config.toml` — written by `quorum link`, read by `quorum review`.
//!
//! Schema:
//! ```toml
//! project_id = "<lippa project id>"
//! base_url = "https://app.lippa.ai"
//! remote_url = true   # optional; default true. If `false`, archive omits remote_url.
//!
//! [memory]                          # Phase 1C — all keys optional
//! candidate_threshold = 3           # 2..=100; auto-promote N
//! local_convention_bundle_cap = 500 # 100..=2048; per-entry bundle bytes
//! candidate_expire_days = 90        # 0..=3650; 0 disables auto-expire
//!
//! [bundle]                          # v0.4 — all keys optional
//! total_budget_kb = 200             # 100..=1024; sections scale proportionally
//! related_files = true              # false restores v0.3.3 candidate selection
//! related_file_max = 12             # 0..=64
//! include = []                      # extra globs, always offered as candidates
//! ```

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct QuorumConfig {
    pub project_id: String,
    pub base_url: String,
    #[serde(default = "default_true")]
    pub remote_url: bool,
    /// Phase 1C — `[memory]` section. Absent in `config.toml` falls back
    /// to [`MemoryConfig::default`] (no warning, per spec §3.4).
    #[serde(default)]
    pub memory: MemoryConfig,
    /// v0.4 — `[bundle]` section. Absent falls back to
    /// [`BundleConfig::default`] with no warning, matching the `[memory]`
    /// precedent (AC 191).
    #[serde(default)]
    pub bundle: BundleConfig,
}

/// v0.4 §4.4 — `[bundle]` section keys. Ranges are hard: out-of-range
/// values surface as [`ConfigError::OutOfRange`] and are never clamped
/// (AC 192).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct BundleConfig {
    #[serde(default = "default_total_budget_kb")]
    pub total_budget_kb: u32,
    #[serde(default = "default_related_files")]
    pub related_files: bool,
    #[serde(default = "default_related_file_max")]
    pub related_file_max: u32,
    /// Extra globs matched against repo-relative paths; matched files
    /// enter as candidates at the class their path implies (AC 193).
    #[serde(default)]
    pub include: Vec<String>,
}

impl Default for BundleConfig {
    fn default() -> Self {
        BundleConfig {
            total_budget_kb: default_total_budget_kb(),
            related_files: default_related_files(),
            related_file_max: default_related_file_max(),
            include: Vec::new(),
        }
    }
}

const fn default_total_budget_kb() -> u32 {
    200
}
const fn default_related_files() -> bool {
    true
}
const fn default_related_file_max() -> u32 {
    12
}

/// Inclusive hard ranges from spec §4.4.
const TOTAL_BUDGET_KB_RANGE: (u32, u32) = (100, 1024);
const RELATED_FILE_MAX_RANGE: (u32, u32) = (0, 64);

/// The default `total_budget_kb`. Above this, `quorum review` warns that
/// an oversized bundle is not rejected by Lippa — it reserves credits,
/// runs, and comes back `failed` (AC 194a).
pub const TOTAL_BUDGET_KB_WARN_ABOVE: u32 = 200;

impl BundleConfig {
    pub fn validate(&self) -> Result<(), String> {
        check_range_in(
            "bundle",
            "total_budget_kb",
            self.total_budget_kb,
            TOTAL_BUDGET_KB_RANGE,
        )?;
        check_range_in(
            "bundle",
            "related_file_max",
            self.related_file_max,
            RELATED_FILE_MAX_RANGE,
        )?;
        Ok(())
    }
}

fn default_true() -> bool {
    true
}

/// Phase 1C — `[memory]` section keys. All keys carry hard ranges per
/// spec §3.4 / §5.4; values outside the range surface as
/// [`ConfigError::OutOfRange`] (exit 2 at the CLI).
#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize)]
pub struct MemoryConfig {
    #[serde(default = "default_candidate_threshold")]
    pub candidate_threshold: u32,
    #[serde(default = "default_local_convention_bundle_cap")]
    pub local_convention_bundle_cap: u32,
    #[serde(default = "default_candidate_expire_days")]
    pub candidate_expire_days: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        MemoryConfig {
            candidate_threshold: default_candidate_threshold(),
            local_convention_bundle_cap: default_local_convention_bundle_cap(),
            candidate_expire_days: default_candidate_expire_days(),
        }
    }
}

const fn default_candidate_threshold() -> u32 {
    3
}
const fn default_local_convention_bundle_cap() -> u32 {
    500
}
const fn default_candidate_expire_days() -> u32 {
    90
}

/// Inclusive hard ranges from spec §5.4. Validation runs after TOML parse;
/// out-of-range values fail rather than clamping (user-visible error).
const CANDIDATE_THRESHOLD_RANGE: (u32, u32) = (2, 100);
const LOCAL_CONVENTION_BUNDLE_CAP_RANGE: (u32, u32) = (100, 2048);
const CANDIDATE_EXPIRE_DAYS_RANGE: (u32, u32) = (0, 3650);

impl MemoryConfig {
    /// Returns `Err` with a human-readable message for the first key whose
    /// value falls outside its hard range. Called from [`read`] and from
    /// `quorum link` so the user sees the error at the write site too.
    pub fn validate(&self) -> Result<(), String> {
        check_range(
            "candidate_threshold",
            self.candidate_threshold,
            CANDIDATE_THRESHOLD_RANGE,
        )?;
        check_range(
            "local_convention_bundle_cap",
            self.local_convention_bundle_cap,
            LOCAL_CONVENTION_BUNDLE_CAP_RANGE,
        )?;
        check_range(
            "candidate_expire_days",
            self.candidate_expire_days,
            CANDIDATE_EXPIRE_DAYS_RANGE,
        )?;
        Ok(())
    }
}

fn check_range(key: &str, value: u32, range: (u32, u32)) -> Result<(), String> {
    check_range_in("memory", key, value, range)
}

fn check_range_in(
    section: &str,
    key: &str,
    value: u32,
    (lo, hi): (u32, u32),
) -> Result<(), String> {
    if value < lo || value > hi {
        Err(format!(
            "[{section}] {key} = {value} is outside the allowed range {lo}..={hi}"
        ))
    } else {
        Ok(())
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("config not found at {0}; run `quorum link --project <id>`")]
    NotFound(std::path::PathBuf),
    #[error("config read failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse failed: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("config serialize failed: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// Phase 1C — `[memory]` range violation (AC 164). The message
    /// includes the offending key, its value, and the allowed range.
    #[error("config out of range: {0}")]
    OutOfRange(String),
    /// SEC-1 — `base_url` is not `https` (or `http` on a loopback host).
    #[error("{0}")]
    InsecureBaseUrl(String),
}

/// SEC-1 — `base_url` is chosen by the repository, so it must not be able
/// to route the session cookie and the bundle over cleartext. `https` is
/// required; plain `http` is accepted only on an explicit loopback host
/// (`localhost`, `127.0.0.1`, `::1`) for local Lippa servers and test
/// doubles. The list is deliberately literal: not `127.0.0.0/8`, not
/// `*.localhost`, and never a DNS lookup.
pub fn validate_base_url(base_url: &str) -> Result<(), String> {
    let allowed = "use https (plain http is accepted only for localhost, 127.0.0.1 and ::1)";
    let url = url::Url::parse(base_url)
        .map_err(|e| format!("base_url = {base_url:?} is not a valid URL ({e}); {allowed}"))?;
    match url.scheme() {
        "https" if url.host().is_some() => Ok(()),
        "http" if is_loopback_host(&url) => Ok(()),
        "http" => Err(format!(
            "base_url = {base_url:?} uses plain http to a non-loopback host, which would send \
             the session cookie and the review bundle in cleartext; {allowed}"
        )),
        other => Err(format!(
            "base_url = {base_url:?} has scheme {other:?}; {allowed}"
        )),
    }
}

fn is_loopback_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip == std::net::Ipv4Addr::LOCALHOST,
        Some(url::Host::Ipv6(ip)) => ip == std::net::Ipv6Addr::LOCALHOST,
        None => false,
    }
}

/// SEC-1 — env var naming the host `QUORUM_LIPPA_SESSION` belongs to. It is
/// required whenever the session is set. Unlike a keyring entry, the env-var
/// cookie is not host-scoped, so without a pin it would follow whatever host
/// the repository's config names.
pub const SESSION_PIN_ENV: &str = "QUORUM_LIPPA_BASE_URL";

/// SEC-1 layer 2 — may the `QUORUM_LIPPA_SESSION` cookie be sent to
/// `base_url`? Only when `pin` names the same host. Hosts are compared
/// exactly as the keyring scopes entries (`host_str`, case-insensitive,
/// port ignored). An unset pin is refused just like a mismatched one, and
/// so is a pin that does not parse as a URL with a host; none of these
/// falls back to sending. `Err` carries the message to show, naming the
/// variable and the host(s).
pub fn check_session_pin(base_url: &str, pin: Option<&str>) -> Result<(), String> {
    let host_of = |u: &str| {
        url::Url::parse(u)
            .ok()
            .and_then(|p| p.host_str().map(str::to_ascii_lowercase))
    };
    let cfg_host =
        host_of(base_url).ok_or_else(|| format!("base_url = {base_url:?} names no host"))?;
    let pin = match pin {
        None => {
            return Err(format!(
                "refusing to send QUORUM_LIPPA_SESSION to {cfg_host}, the host named by base_url \
                 in this repository's .quorum/config.toml: {SESSION_PIN_ENV} is not set. Set \
                 {SESSION_PIN_ENV} to the Lippa URL the session belongs to (e.g. \
                 {SESSION_PIN_ENV}=https://app.lippa.ai); the session is then sent only to that \
                 host. Or unset QUORUM_LIPPA_SESSION to use a login stored by `quorum auth login`."
            ))
        }
        Some(p) => p,
    };
    let pin_host = host_of(pin).ok_or_else(|| {
        format!(
            "{SESSION_PIN_ENV} = {pin:?} is not a URL with a host \
             (expected e.g. https://app.lippa.ai); refusing to send QUORUM_LIPPA_SESSION"
        )
    })?;
    if pin_host == cfg_host {
        Ok(())
    } else {
        Err(format!(
            "refusing to send QUORUM_LIPPA_SESSION to {cfg_host}: .quorum/config.toml base_url \
             names host {cfg_host}, but {SESSION_PIN_ENV} pins {pin_host}"
        ))
    }
}

pub fn read(repo_root: &Path) -> Result<QuorumConfig, ConfigError> {
    let path = repo_root.join(".quorum").join("config.toml");
    if !path.exists() {
        return Err(ConfigError::NotFound(path));
    }
    let text = std::fs::read_to_string(&path)?;
    let cfg = toml::from_str::<QuorumConfig>(&text)?;
    // SEC-1 — checked at load, so every command that reads the config
    // fails the same way (exit 2), not only `quorum review`.
    validate_base_url(&cfg.base_url).map_err(ConfigError::InsecureBaseUrl)?;
    cfg.memory.validate().map_err(ConfigError::OutOfRange)?;
    cfg.bundle.validate().map_err(ConfigError::OutOfRange)?;
    Ok(cfg)
}

pub fn write(repo_root: &Path, cfg: &QuorumConfig) -> Result<std::path::PathBuf, ConfigError> {
    // SEC-1 — refuse at the write site too, rather than writing a config
    // that every later command would reject.
    validate_base_url(&cfg.base_url).map_err(ConfigError::InsecureBaseUrl)?;
    let dir = repo_root.join(".quorum");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    let text = toml::to_string_pretty(cfg)?;
    std::fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip() {
        let dir = tempdir().unwrap();
        let cfg = QuorumConfig {
            project_id: "p_1".into(),
            base_url: "https://app.lippa.ai".into(),
            remote_url: true,
            memory: MemoryConfig::default(),
            bundle: BundleConfig::default(),
        };
        let path = write(dir.path(), &cfg).unwrap();
        assert!(path.exists());
        let got = read(dir.path()).unwrap();
        assert_eq!(got.project_id, "p_1");
        assert!(got.remote_url);
    }

    #[test]
    fn missing_config_errors_with_helpful_message() {
        let dir = tempdir().unwrap();
        match read(dir.path()) {
            Err(ConfigError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    fn write_raw(dir: &std::path::Path, body: &str) {
        let qd = dir.join(".quorum");
        std::fs::create_dir_all(&qd).unwrap();
        std::fs::write(qd.join("config.toml"), body).unwrap();
    }

    #[test]
    fn memory_defaults_when_section_absent() {
        // AC 165: default config (no [memory] section) yields the
        // documented defaults: threshold=3, cap=500, expire=90.
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p_1\"\nbase_url = \"https://app.lippa.ai\"\n",
        );
        let cfg = read(dir.path()).unwrap();
        assert_eq!(cfg.memory.candidate_threshold, 3);
        assert_eq!(cfg.memory.local_convention_bundle_cap, 500);
        assert_eq!(cfg.memory.candidate_expire_days, 90);
    }

    #[test]
    fn memory_explicit_values_parsed() {
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p_1\"\nbase_url = \"https://app.lippa.ai\"\n\
             [memory]\n\
             candidate_threshold = 5\n\
             local_convention_bundle_cap = 1024\n\
             candidate_expire_days = 30\n",
        );
        let cfg = read(dir.path()).unwrap();
        assert_eq!(cfg.memory.candidate_threshold, 5);
        assert_eq!(cfg.memory.local_convention_bundle_cap, 1024);
        assert_eq!(cfg.memory.candidate_expire_days, 30);
    }

    #[test]
    fn memory_threshold_out_of_range_rejected() {
        // AC 164: candidate_threshold = 0 / 1 / 101 — all rejected.
        for bad in [0u32, 1, 101] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
                     [memory]\ncandidate_threshold = {bad}\n"
                ),
            );
            match read(dir.path()) {
                Err(ConfigError::OutOfRange(m)) => {
                    assert!(
                        m.contains("candidate_threshold"),
                        "error message must name the key, got: {m}"
                    );
                }
                other => panic!("expected OutOfRange for threshold={bad}, got {other:?}"),
            }
        }
        // Boundary OK: 2 and 100 pass.
        for ok in [2u32, 100] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
                     [memory]\ncandidate_threshold = {ok}\n"
                ),
            );
            read(dir.path()).expect("boundary value must pass");
        }
    }

    #[test]
    fn memory_bundle_cap_out_of_range_rejected() {
        // AC 164: local_convention_bundle_cap range 100..=2048.
        for bad in [0u32, 99, 2049, 10_000] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
                     [memory]\nlocal_convention_bundle_cap = {bad}\n"
                ),
            );
            match read(dir.path()) {
                Err(ConfigError::OutOfRange(m)) => {
                    assert!(m.contains("local_convention_bundle_cap"));
                }
                other => panic!("expected OutOfRange for cap={bad}, got {other:?}"),
            }
        }
    }

    #[test]
    fn memory_expire_days_out_of_range_rejected() {
        // AC 164: candidate_expire_days range 0..=3650 (0 disables).
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
             [memory]\ncandidate_expire_days = 3651\n",
        );
        match read(dir.path()) {
            Err(ConfigError::OutOfRange(m)) => {
                assert!(m.contains("candidate_expire_days"));
            }
            other => panic!("expected OutOfRange, got {other:?}"),
        }
        // 0 is explicitly allowed (disables auto-expire per spec §3.4).
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
             [memory]\ncandidate_expire_days = 0\n",
        );
        read(dir.path()).expect("0 is the disable sentinel and must be allowed");
    }

    // ===================== v0.4 `[bundle]` section =====================

    /// AC 191 — all four keys optional; an absent `[bundle]` section
    /// yields the documented defaults with no warning, exactly as the
    /// `[memory]` precedent does.
    #[test]
    fn ac191_bundle_defaults_when_section_absent() {
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p_1\"\nbase_url = \"https://app.lippa.ai\"\n",
        );
        let cfg = read(dir.path()).unwrap();
        assert_eq!(cfg.bundle.total_budget_kb, 200);
        assert!(cfg.bundle.related_files);
        assert_eq!(cfg.bundle.related_file_max, 12);
        assert!(cfg.bundle.include.is_empty());
    }

    /// AC 191 — a partial `[bundle]` section fills the rest from defaults.
    #[test]
    fn ac191_bundle_partial_section_fills_defaults() {
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n[bundle]\nrelated_files = false\n",
        );
        let cfg = read(dir.path()).unwrap();
        assert!(!cfg.bundle.related_files);
        assert_eq!(cfg.bundle.total_budget_kb, 200);
        assert_eq!(cfg.bundle.related_file_max, 12);
    }

    #[test]
    fn ac191_bundle_explicit_values_parsed() {
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
             [bundle]\n\
             total_budget_kb = 512\n\
             related_files = false\n\
             related_file_max = 64\n\
             include = [\"schema/*.sql\", \"**/*.proto\"]\n",
        );
        let cfg = read(dir.path()).unwrap();
        assert_eq!(cfg.bundle.total_budget_kb, 512);
        assert!(!cfg.bundle.related_files);
        assert_eq!(cfg.bundle.related_file_max, 64);
        assert_eq!(cfg.bundle.include, vec!["schema/*.sql", "**/*.proto"]);
    }

    /// AC 192 — out-of-range values produce `OutOfRange`; never clamped.
    #[test]
    fn ac192_total_budget_kb_out_of_range_rejected() {
        for bad in [0u32, 1, 99, 1025, 100_000] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
                     [bundle]\ntotal_budget_kb = {bad}\n"
                ),
            );
            match read(dir.path()) {
                Err(ConfigError::OutOfRange(m)) => {
                    assert!(
                        m.contains("total_budget_kb") && m.contains("[bundle]"),
                        "error must name the section and key, got: {m}"
                    );
                }
                other => panic!("expected OutOfRange for {bad}, got {other:?}"),
            }
        }
        // Boundaries pass, and are not silently rewritten.
        for ok in [100u32, 1024] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
                     [bundle]\ntotal_budget_kb = {ok}\n"
                ),
            );
            let cfg = read(dir.path()).expect("boundary value must pass");
            assert_eq!(cfg.bundle.total_budget_kb, ok, "value must not be clamped");
        }
    }

    /// AC 192 — `related_file_max` range 0..=64; 0 is a legal "no related
    /// files" setting, not an error.
    #[test]
    fn ac192_related_file_max_out_of_range_rejected() {
        for bad in [65u32, 100, 10_000] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
                     [bundle]\nrelated_file_max = {bad}\n"
                ),
            );
            match read(dir.path()) {
                Err(ConfigError::OutOfRange(m)) => {
                    assert!(m.contains("related_file_max"), "got: {m}");
                }
                other => panic!("expected OutOfRange for {bad}, got {other:?}"),
            }
        }
        for ok in [0u32, 64] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"https://app.lippa.ai\"\n\
                     [bundle]\nrelated_file_max = {ok}\n"
                ),
            );
            let cfg = read(dir.path()).expect("boundary must pass");
            assert_eq!(cfg.bundle.related_file_max, ok);
        }
    }

    /// The `[bundle]` section survives a write/read round trip, so
    /// `quorum link` never silently drops a user's settings.
    #[test]
    fn bundle_config_round_trips() {
        let dir = tempdir().unwrap();
        let cfg = QuorumConfig {
            project_id: "p_1".into(),
            base_url: "https://app.lippa.ai".into(),
            remote_url: true,
            memory: MemoryConfig::default(),
            bundle: BundleConfig {
                total_budget_kb: 300,
                related_files: false,
                related_file_max: 5,
                include: vec!["schema/*.sql".into()],
            },
        };
        write(dir.path(), &cfg).unwrap();
        let got = read(dir.path()).unwrap();
        assert_eq!(got.bundle.total_budget_kb, 300);
        assert!(!got.bundle.related_files);
        assert_eq!(got.bundle.related_file_max, 5);
        assert_eq!(got.bundle.include, vec!["schema/*.sql"]);
    }

    // ===================== SEC-1 — base_url scheme + session pin =====================

    fn config_with_base_url(base_url: &str) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            &format!("project_id = \"p\"\nbase_url = \"{base_url}\"\n"),
        );
        dir
    }

    /// An `http://` base_url on a non-loopback host is rejected at load,
    /// along with every other non-https scheme and anything unparseable.
    #[test]
    fn sec1_insecure_base_url_rejected_at_load() {
        for bad in [
            "http://evil.example",
            "http://app.lippa.ai",
            "http://app.lippa.ai:443/",
            "http://localhost.evil.example",
            "http://127.0.0.2",
            "http://10.0.0.1:8000",
            "ftp://app.lippa.ai",
            "file:///etc/passwd",
            "app.lippa.ai",
            "u",
            "",
        ] {
            let dir = config_with_base_url(bad);
            match read(dir.path()) {
                Err(ConfigError::InsecureBaseUrl(m)) => {
                    assert!(m.contains("https"), "message must say what is allowed: {m}");
                }
                other => panic!("expected InsecureBaseUrl for {bad:?}, got {other:?}"),
            }
        }
    }

    /// https is accepted on any host; plain http only on the three
    /// explicit loopback spellings.
    #[test]
    fn sec1_https_and_loopback_http_accepted_at_load() {
        for ok in [
            "https://app.lippa.ai",
            "https://staging.lippa.example:8443/base",
            "http://localhost",
            "http://LOCALHOST:8000",
            "http://127.0.0.1:1",
            "http://[::1]:9000",
        ] {
            let dir = config_with_base_url(ok);
            let cfg = read(dir.path()).unwrap_or_else(|e| panic!("{ok:?} rejected: {e}"));
            assert_eq!(cfg.base_url, ok, "value must not be rewritten");
        }
    }

    /// `quorum link` cannot write a config that every command would reject.
    #[test]
    fn sec1_insecure_base_url_rejected_at_write() {
        let dir = tempdir().unwrap();
        let cfg = QuorumConfig {
            project_id: "p".into(),
            base_url: "http://evil.example".into(),
            remote_url: true,
            memory: MemoryConfig::default(),
            bundle: BundleConfig::default(),
        };
        assert!(matches!(
            write(dir.path(), &cfg),
            Err(ConfigError::InsecureBaseUrl(_))
        ));
        assert!(!dir.path().join(".quorum").join("config.toml").exists());
    }

    #[test]
    fn sec1_session_pin_outcomes() {
        // No pin, no env cookie: the refusal names the variable to set and
        // the host the repository asked for.
        let err = check_session_pin("https://evil.example", None).expect_err("unset pin refuses");
        assert!(
            err.contains(SESSION_PIN_ENV) && err.contains("evil.example"),
            "must name the variable and the host: {err}"
        );
        // Host-only, as the keyring scopes entries: scheme, port and path
        // do not matter, case does not matter.
        assert_eq!(
            check_session_pin("https://app.lippa.ai", Some("https://APP.lippa.ai:8443/x")),
            Ok(())
        );
        let err = check_session_pin("https://evil.example", Some("https://app.lippa.ai"))
            .expect_err("mismatch must refuse");
        assert!(
            err.contains("evil.example") && err.contains("app.lippa.ai"),
            "both hosts must be named: {err}"
        );
        assert!(err.contains(SESSION_PIN_ENV));
        // A pin that is not a URL is an error, never silently ignored.
        let err = check_session_pin("https://app.lippa.ai", Some("app.lippa.ai")).unwrap_err();
        assert!(err.contains(SESSION_PIN_ENV), "{err}");
    }
}
