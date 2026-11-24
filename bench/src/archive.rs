//! The publishable raw-output archive (AC 247): every file of a run
//! directory, scrubbed by the same filter that guards manifests, in a
//! deterministic `.tar.gz`, with its SHA-256 printed. Rolf creates any
//! `bench-<run-id>` tag and release by hand.

use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::scrub::Scrubber;

#[derive(thiserror::Error, Debug)]
pub enum ArchiveError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("`{0}` is not UTF-8 text; the archive holds only scrubbable text files")]
    NotText(String),
}

fn files(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if e.file_type()?.is_dir() {
            files(&e.path(), &r, out)?;
        } else {
            out.push((r, e.path()));
        }
    }
    Ok(())
}

/// Write `<run_dir>` as a scrubbed archive at `out`. Entries are sorted,
/// with zero mtimes and fixed modes, so the same input gives the same
/// bytes. Returns the archive's SHA-256.
pub fn write_archive(
    run_dir: &Path,
    prefix: &str,
    out: &Path,
    scrubber: &Scrubber,
) -> Result<String, ArchiveError> {
    let mut list = Vec::new();
    files(run_dir, "", &mut list)?;
    list.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    for (rel, path) in list {
        let bytes = std::fs::read(&path)?;
        let text = String::from_utf8(bytes).map_err(|_| ArchiveError::NotText(rel.clone()))?;
        let clean = scrubber.scrub(&text);
        let mut h = tar::Header::new_gnu();
        h.set_size(clean.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(0);
        h.set_cksum();
        tar.append_data(&mut h, format!("{prefix}/{rel}"), clean.as_bytes())?;
    }
    let gz = tar.into_inner()?;
    let bytes = gz.finish()?;
    let mut f = std::fs::File::create(out)?;
    f.write_all(&bytes)?;
    Ok(crate::corpus::sha256_hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn archive_is_scrubbed_and_deterministic() {
        let td = tempfile::TempDir::new().unwrap();
        let run = td.path().join("run");
        std::fs::create_dir_all(run.join("cases/c1")).unwrap();
        let cookie = format!("cookievalue{}", "x".repeat(20));
        std::fs::write(
            run.join("manifest.json"),
            format!("{{\"note\": \"{cookie}\", \"ok\": 1}}"),
        )
        .unwrap();
        std::fs::write(
            run.join("cases/c1/stderr.txt"),
            // Built at runtime: the key guard scans this tracked file.
            format!(
                "Authorization: {} abcdefghijklmnopqrstuvwxyz\nplain\n",
                "Bearer"
            ),
        )
        .unwrap();
        let s = Scrubber::new(vec![cookie.clone()]);
        let a = td.path().join("a.tar.gz");
        let b = td.path().join("b.tar.gz");
        let ha = write_archive(&run, "bench-x", &a, &s).unwrap();
        let hb = write_archive(&run, "bench-x", &b, &s).unwrap();
        assert_eq!(ha, hb, "same input, same bytes");
        let mut gz = flate2::read::GzDecoder::new(std::fs::File::open(&a).unwrap());
        let mut all = Vec::new();
        gz.read_to_end(&mut all).unwrap();
        let text = String::from_utf8_lossy(&all);
        assert!(!text.contains(&cookie));
        assert!(!text.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(text.contains("plain"));
        assert!(text.contains("bench-x/cases/c1/stderr.txt"));
    }

    #[test]
    fn binary_files_are_refused() {
        let td = tempfile::TempDir::new().unwrap();
        let outdir = tempfile::TempDir::new().unwrap();
        std::fs::write(td.path().join("x.bin"), [0xff, 0xfe, 0x00]).unwrap();
        let e = write_archive(
            td.path(),
            "p",
            &outdir.path().join("o.tgz"),
            &Scrubber::default(),
        );
        assert!(matches!(e, Err(ArchiveError::NotText(_))));
    }
}
