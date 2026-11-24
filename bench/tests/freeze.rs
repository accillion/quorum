//! The design freeze (spec §3): every frozen file's SHA-256 in
//! bench/DESIGN.md matches the file, the corpus manifest is fresh, and the
//! frozen settings are internally consistent. A failure here means the
//! design changed after the freeze, which restarts the pilot.

use std::path::{Path, PathBuf};

use quorum_bench::commands::{file_sha, frozen_files};
use quorum_bench::config::BenchConfig;
use quorum_bench::prices::PriceTable;
use quorum_bench::templates::Template;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn design() -> String {
    std::fs::read_to_string(root().join("bench/DESIGN.md")).unwrap()
}

#[test]
fn design_md_pins_every_frozen_file() {
    let root = root();
    let cfg = BenchConfig::load(&root.join("bench/bench.toml")).unwrap();
    let d = design();
    for f in frozen_files(&cfg) {
        let sha = file_sha(&root, &f).unwrap();
        let row = d
            .lines()
            .find(|l| l.contains(&format!("`{f}`")))
            .unwrap_or_else(|| panic!("DESIGN.md has no row for {f}"));
        assert!(
            row.contains(&sha),
            "DESIGN.md pins a different SHA-256 for {f}; the file is now {sha}"
        );
    }
}

#[test]
fn the_corpus_manifest_is_fresh() {
    let root = root();
    let cases = quorum_bench::corpus::load_corpus(&root.join("bench/cases")).unwrap();
    let text = quorum_bench::corpus::manifest_text(&cases).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("bench/MANIFEST.toml")).unwrap(),
        text,
        "run `quorum-bench manifest`"
    );
    assert!(quorum_bench::corpus::corpus_problems(&cases).is_empty());
}

#[test]
fn frozen_settings_are_consistent() {
    let root = root();
    let cfg = BenchConfig::load(&root.join("bench/bench.toml")).unwrap();
    let prices = PriceTable::load(&root.join("bench/prices.toml")).unwrap();
    for m in cfg.s_arms.models.values() {
        prices.get(&m.vendor_model).unwrap();
    }
    prices.get(&cfg.judge.model).unwrap();
    assert!(cfg.lippa.base_url.starts_with("https://"));
    assert_eq!(cfg.run.repeats, 3, "AC 249");
    assert_eq!(
        cfg.s_arms.output_cap, 16_384,
        "Gate A ruling 4: the seats' ceiling"
    );
    assert_eq!(cfg.adjudication.human_share, 0.2);
    assert_eq!(cfg.quorum.commit.len(), 40);
    for f in [
        &cfg.s_arms.prompt,
        &cfg.judge.rubric,
        &cfg.extraction.rubric,
        &cfg.clustering.rubric,
    ] {
        Template::load(&root.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    }
    // The S- prompt is Lippa's Round 0 wrapper with only the multi-model
    // sentences removed.
    let single = Template::load(&root.join(&cfg.s_arms.prompt)).unwrap();
    assert!(!single.system.contains("multi-model") && !single.system.contains("other models"));
    assert!(single
        .system
        .starts_with("<lippa_safety version=\"2026-10-30.1\">"));
    assert!(
        single.system.contains("---CLAIMS---")
            && single
                .system
                .contains("Provide your independent assessment.")
    );
    assert_eq!(single.user, "Question:\n{{BUNDLE}}");
}
