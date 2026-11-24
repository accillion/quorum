# Benchmark corpus format (v0.6 Stage 1, ACs 242–246)

Every case lives at `bench/cases/<case-id>/`. The harness never runs
anything from a case directory: it reads the files as data.

`bench/cases/**` is stored byte-exact (`-text` in `.gitattributes`), so a
Windows checkout with `core.autocrlf = true` cannot change a patch or a
hash.

## Files

| File | Bug case | Clean case |
|---|---|---|
| `case.toml` | required | required |
| `diff.patch` | the fix reverted, non-test files only | the commit, non-test files only |
| `expected.toml` | required | absent |
| `repro/` | required (AC 244) | absent |

## Trees

- **Bug case.** The case base is the tree of `base_commit` (the upstream
  commit that contains the fix) with every path in `removed_paths` deleted
  and every path in `base_reverts` set back to its content in the fix's
  parent. `removed_paths` lists every test file (and test snapshot) that
  the fix added or changed. `diff.patch` is the reverse of the fix's
  changes to every other file, so the case commit reintroduces the bug.
- **Clean case.** The case base is the tree of `base_commit` (the clean
  commit's parent). `diff.patch` is the clean commit's own non-test
  changes. `removed_paths` and `base_reverts` are normally empty.
- `case_base_tree` and `case_commit_tree` pin both trees. The materializer
  rebuilds them with git2 and refuses a case whose hashes differ.

Corpus rules that keep the reviewer from seeing the answer:

1. A fix whose test sits **inside a changed source file** (for example a
   Rust `#[cfg(test)] mod tests` block in the fixed file) is not used: its
   reversal would show the deleted test in the diff.
2. Changelog and release-note edits are never part of `diff.patch`.
   An added changeset or news fragment is in `removed_paths`. An edit to
   a cumulative changelog is in `base_reverts`.
3. `diff.patch` touches at most 5 files and 300 changed lines, so the
   bundle fits the default 200 KB budget with no cut (AC 243). The harness
   checks the real bundle.
4. No secrets and no personal data beyond the licence's copyright line
   (AC 245).

## `case.toml`

```toml
schema = 1
id = "rs-oxc-01"
kind = "bug"                     # bug | clean
language = "rust"                # rust | typescript | python
source_repo = "https://github.com/<owner>/<repo>"
source_commit = "<40 hex>"       # bug: the commit that merged the fix; clean: the clean commit
source_commit_url = "https://github.com/<owner>/<repo>/commit/<sha>"
source_commit_date = "2026-03-01"   # committer date, UTC
base_commit = "<40 hex>"         # bug: = source_commit; clean: its first parent
removed_paths = []               # repo-relative, forward slashes
base_reverts = []                # repo-relative, forward slashes
case_base_tree = "<40 hex>"
case_commit_tree = "<40 hex>"
licence = "MIT"                  # MIT | Apache-2.0 | BSD-2-Clause | BSD-3-Clause | MIT OR Apache-2.0
licence_path = "LICENSE"         # at base_commit
licence_notice = """
Copyright (c) ...
"""
agent_files = ["AGENTS.md"]      # AGENTS.md / CLAUDE.md present at the case base root; [] if none
dataset = "SWE-bench-Live/MultiLang"   # or "hand-picked"
dataset_instance = ""            # the dataset's instance id, "" when hand-picked

[repro]                          # bug cases only
reproduced = true
image = "<docker image, by digest>"
command = "<command run from the repo root inside the container>"
test_files = ["<repo path>"]     # the files under repro/files/, at their repo paths
fixed_exit = 0                   # exit status on the fixed tree + test files
reverted_exit = 101              # exit status on the case commit tree + test files

[clean]                          # clean cases only (AC 243)
head_checked = "<40 hex>"        # the upstream commit the history check ran against
head_checked_date = "2026-11-23"
retained_share = 0.95            # share of the commit's added non-test lines still blamed to it at head_checked
later_commits = []               # later commits touching those lines: "<sha> <subject>"
verdict = "no later fix, revert or substantial rewrite"
```

## `expected.toml` (bug cases)

```toml
[[defect]]
id = "D1"
severity = "high"                # high | medium | low: impact of the reintroduced bug
files = ["src/foo.rs"]
title = "One sentence, at most 20 words"
description = """
What is wrong and how it fails, written from the fix commit and its
issue. Never from the reproduction output.
"""
```

## `repro/`

```
repro/files/<repo path>    # each test file the fix added or changed, as in the fix commit
repro/run.sh               # the command in case.toml [repro], for reference
repro/output-fixed.txt     # last 200 lines of the run on the fixed tree
repro/output-reverted.txt  # last 200 lines of the run on the case commit tree
```

`reproduced = true` only when the run fails on the case commit tree and
passes on the fixed tree, both with the `repro/files` present. A case
that cannot be reproduced keeps `reproduced = false` and is reported
separately (AC 244).

## Content hash

`bench/MANIFEST.toml` lists every case with the SHA-256 of its directory:
the files in byte order of their forward-slash relative paths, each fed as
`path`, a NUL byte, the 8-byte little-endian length, then the bytes.
