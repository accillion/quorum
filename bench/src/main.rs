//! `quorum-bench`: the v0.6 Stage 1 benchmark harness CLI.
//!
//! Every command first runs the key guard (AC 247) and refuses to start if
//! a tracked file under `bench/` holds a key-shaped value.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use quorum_bench::{archive, corpus, keyguard, runid, scrub::Scrubber, secret::KeySource, verify};

#[derive(Parser)]
#[command(
    name = "quorum-bench",
    about = "Quorum benchmark harness (not shipped)"
)]
struct Cli {
    /// Repository root (default: discovered from the current directory).
    #[arg(long, global = true)]
    repo_root: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run only the key guard.
    CheckKeys,
    /// Validate, materialize and leak-check cases; with --quorum-bin, also
    /// a zero-cost dry run against a local fake Lippa.
    VerifyCorpus {
        #[arg(long)]
        case: Option<String>,
        #[arg(long)]
        quorum_bin: Option<PathBuf>,
    },
    /// Write bench/MANIFEST.toml (or, with --check, fail if it is stale).
    Manifest {
        #[arg(long)]
        check: bool,
    },
    /// Write the scrubbed archive of bench/runs/<run-id> and print its SHA-256.
    Archive {
        #[arg(long)]
        run: String,
    },
    /// Print a fresh run id.
    NewRunId,
    /// The paid pilot run (1b). Both caps are required (AC 250).
    Run {
        /// Lippa credits cap.
        #[arg(long)]
        max_credits: u64,
        /// USD cap for direct, extraction, clustering and judge calls.
        #[arg(long)]
        max_usd: String,
        /// The `quorum` binary built from bench.toml's pinned commit.
        #[arg(long)]
        quorum_bin: PathBuf,
        #[arg(long)]
        case: Option<String>,
        /// Required: the run spends money.
        #[arg(long)]
        confirm_paid: bool,
    },
    /// Gate B ruling 2: remove the fix's comment-only lines from bug cases'
    /// bases and regenerate their diff.patch (corpus maintenance).
    StripCommentHints {
        #[arg(long)]
        case: Option<String>,
    },
    /// (Re)build the blind human queue of a finished run.
    Queue {
        #[arg(long)]
        run: String,
    },
    /// Decide the queued items one at a time, blind to arm and model.
    Adjudicate {
        #[arg(long)]
        run: String,
    },
    /// Score a finished, fully adjudicated run and compute C1 and C2.
    Results {
        #[arg(long)]
        run: String,
        #[arg(long, default_value_t = quorum_bench::stats::DEFAULT_DRAWS)]
        draws: usize,
    },
}

fn repo_root(arg: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(p) = arg {
        return Ok(p);
    }
    let repo =
        git2::Repository::discover(".").map_err(|e| format!("not in a git repository: {e}"))?;
    repo.workdir()
        .map(Path::to_path_buf)
        .ok_or_else(|| "bare repository".to_string())
}

fn run(cli: Cli) -> Result<(), String> {
    let root = repo_root(cli.repo_root)?;
    let bench = root.join("bench");
    let keys = KeySource::load(&bench).map_err(|e| format!("bench/.env.local: {e}"))?;
    let hits = keyguard::check_repo(&root, &keys.known_values()).map_err(|e| e.to_string())?;
    if !hits.is_empty() {
        for h in &hits {
            eprintln!(
                "key guard: {}:{}: {} (sha256 {})",
                h.path, h.line, h.kind, h.digest
            );
        }
        return Err(format!(
            "refusing to start: {} key-shaped value(s) in tracked files under bench/ (AC 247)",
            hits.len()
        ));
    }
    let cache = bench.join(".cache");
    match cli.cmd {
        Cmd::CheckKeys => {
            println!("key guard: clean");
            Ok(())
        }
        Cmd::VerifyCorpus { case, quorum_bin } => {
            let cases = corpus::load_corpus(&bench.join("cases")).map_err(|e| e.to_string())?;
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            let mut failed = 0;
            for c in cases
                .iter()
                .filter(|c| case.as_deref().map_or(true, |id| c.id() == id))
            {
                let r = rt.block_on(verify::verify_case(c, &cache, quorum_bin.as_deref()));
                let size = r
                    .bundle_bytes
                    .map(|b| format!("{b} B"))
                    .unwrap_or_else(|| "-".into());
                if r.problems.is_empty() {
                    println!(
                        "ok    {:<28} bundle {size}, context omitted {}",
                        r.id, r.context_files_omitted
                    );
                } else {
                    failed += 1;
                    println!("FAIL  {:<28} {}", r.id, r.problems.join("; "));
                }
                for n in &r.notes {
                    println!("note  {:<28} {n}", r.id);
                }
            }
            for p in corpus::corpus_problems(&cases) {
                failed += 1;
                println!("FAIL  corpus: {p}");
            }
            if failed > 0 {
                Err(format!("{failed} problem(s)"))
            } else {
                Ok(())
            }
        }
        Cmd::Manifest { check } => {
            let cases = corpus::load_corpus(&bench.join("cases")).map_err(|e| e.to_string())?;
            let text = corpus::manifest_text(&cases).map_err(|e| e.to_string())?;
            let path = bench.join("MANIFEST.toml");
            if check {
                let cur = std::fs::read_to_string(&path).unwrap_or_default();
                if cur != text {
                    return Err("bench/MANIFEST.toml is stale: run `quorum-bench manifest`".into());
                }
            } else {
                std::fs::write(&path, &text).map_err(|e| e.to_string())?;
            }
            println!(
                "corpus manifest sha256 {}",
                corpus::sha256_hex(text.as_bytes())
            );
            Ok(())
        }
        Cmd::Archive { run } => {
            if !runid::is_valid_run_id(&run) {
                return Err(format!("`{run}` is not a run id"));
            }
            let dir = bench.join("runs").join(&run);
            let out = bench.join("runs").join(format!("bench-{run}.tar.gz"));
            let sha = archive::write_archive(
                &dir,
                &format!("bench-{run}"),
                &out,
                &Scrubber::new(keys.known_values()),
            )
            .map_err(|e| e.to_string())?;
            println!("{}  {sha}", out.display());
            Ok(())
        }
        Cmd::Run {
            max_credits,
            max_usd,
            quorum_bin,
            case,
            confirm_paid,
        } => {
            let max_usd_micros = quorum_bench::budget::parse_usd(&max_usd)?;
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            rt.block_on(quorum_bench::commands::cmd_run(
                &root,
                &keys,
                quorum_bench::commands::RunOpts {
                    max_credits,
                    max_usd_micros,
                    quorum_bin,
                    case,
                    confirm_paid,
                },
            ))
        }
        Cmd::StripCommentHints { case } => {
            let cases = corpus::load_corpus(&bench.join("cases")).map_err(|e| e.to_string())?;
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            for c in cases.iter().filter(|c| {
                c.meta.kind == corpus::Kind::Bug && case.as_deref().map_or(true, |id| c.id() == id)
            }) {
                // An already-edited case is edited again only if its
                // current diff still shows a hint; the edits accumulate.
                let m = rt
                    .block_on(quorum_bench::materialize::materialize(c, &cache))
                    .map_err(|e| e.to_string())?;
                match quorum_bench::edit::comment_edit(c, &m).map_err(|e| e.to_string())? {
                    None => {}
                    Some(e) => {
                        quorum_bench::edit::apply_to_case(c, &e).map_err(|e| e.to_string())?;
                        println!(
                            "edit  {:<28} {} line(s) removed, {} trailing comment(s) stripped, in {}",
                            c.id(),
                            e.lines_removed,
                            e.trailing_stripped,
                            e.files.join(", ")
                        );
                    }
                }
            }
            Ok(())
        }
        Cmd::Queue { run } => quorum_bench::commands::cmd_queue(&root, &run),
        Cmd::Adjudicate { run } => {
            let dir = bench.join("runs").join(&run);
            let stdin = std::io::stdin();
            let left = quorum_bench::adjudicate::run_session(
                &dir,
                &mut stdin.lock(),
                &mut std::io::stdout(),
            )
            .map_err(|e| e.to_string())?;
            println!("\n{left} item(s) left");
            Ok(())
        }
        Cmd::Results { run, draws } => quorum_bench::commands::cmd_results(&root, &run, draws),
        Cmd::NewRunId => {
            println!("{}", runid::new_run_id(time::OffsetDateTime::now_utc()));
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("quorum-bench: {e}");
            ExitCode::from(2)
        }
    }
}
