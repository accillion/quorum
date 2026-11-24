//! Run ids (AC 247): unique, sortable and dot-free, so that a
//! `bench-<run-id>` tag can never match the release workflow's tag filter.

use rand::Rng;

/// `YYYYMMDDTHHMMSSZ-<6 hex>`, e.g. `20261124T101500Z-a1b2c3`.
pub fn new_run_id(now: time::OffsetDateTime) -> String {
    let fmt = time::macros::format_description!("[year][month][day]T[hour][minute][second]Z");
    let stamp = now
        .to_offset(time::UtcOffset::UTC)
        .format(&fmt)
        .expect("fixed format");
    let suffix: u32 = rand::thread_rng().gen_range(0..0x100_0000);
    format!("{stamp}-{suffix:06x}")
}

pub fn is_valid_run_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 23
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'T'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'Z'
        && b[16] == b'-'
        && b[17..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

/// A matcher for the subset of GitHub Actions filter-pattern syntax that
/// `release.yml` uses: `*` (any run of characters except `/`), `**` (any
/// run of characters), `+` (one or more of the preceding atom), `[...]`
/// character classes with ranges, and literals. `.` is a literal in GitHub
/// filter patterns.
pub fn github_filter_matches(pattern: &str, text: &str) -> bool {
    #[derive(Debug, Clone)]
    enum Atom {
        Lit(char),
        Class(Vec<(char, char)>),
        Star,
        DoubleStar,
    }
    #[derive(Debug, Clone)]
    struct Item {
        atom: Atom,
        plus: bool,
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut items: Vec<Item> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let atom = match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                Atom::DoubleStar
            }
            '*' => {
                i += 1;
                Atom::Star
            }
            '[' => {
                let mut ranges = Vec::new();
                i += 1;
                while i < chars.len() && chars[i] != ']' {
                    if i + 2 < chars.len() && chars[i + 1] == '-' && chars[i + 2] != ']' {
                        ranges.push((chars[i], chars[i + 2]));
                        i += 3;
                    } else {
                        ranges.push((chars[i], chars[i]));
                        i += 1;
                    }
                }
                i += 1;
                Atom::Class(ranges)
            }
            '+' => {
                if let Some(last) = items.last_mut() {
                    last.plus = true;
                }
                i += 1;
                continue;
            }
            c => {
                i += 1;
                Atom::Lit(c)
            }
        };
        items.push(Item { atom, plus: false });
    }
    let text: Vec<char> = text.chars().collect();
    fn one(atom: &Atom, c: char) -> bool {
        match atom {
            Atom::Lit(l) => *l == c,
            Atom::Class(r) => r.iter().any(|&(a, b)| a <= c && c <= b),
            Atom::Star => c != '/',
            Atom::DoubleStar => true,
        }
    }
    fn go(items: &[Item], text: &[char]) -> bool {
        let Some((first, rest)) = items.split_first() else {
            return text.is_empty();
        };
        let (min, unbounded) = match first.atom {
            Atom::Star | Atom::DoubleStar => (0, true),
            _ if first.plus => (1, true),
            _ => (1, false),
        };
        let max = if unbounded { text.len() } else { 1 };
        let mut n = 0;
        while n < min {
            if n >= text.len() || !one(&first.atom, text[n]) {
                return false;
            }
            n += 1;
        }
        loop {
            if go(rest, &text[n..]) {
                return true;
            }
            if n >= max || n >= text.len() || !one(&first.atom, text[n]) {
                return false;
            }
            n += 1;
        }
    }
    go(&items, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_are_valid_unique_and_dot_free() {
        let now = time::OffsetDateTime::from_unix_timestamp(1_795_000_000).unwrap();
        let a = new_run_id(now);
        let b = new_run_id(now);
        assert!(is_valid_run_id(&a), "{a}");
        assert!(!a.contains('.'));
        assert_ne!(a, b, "random suffix");
        assert!(!is_valid_run_id("20261124T101500Z-a1b2c"));
        assert!(!is_valid_run_id("2026.11.24T101500Z-a1b2c3"));
    }

    /// Reads the tag filters from the real release workflow. Positive
    /// controls: the filter matches real release tags. The check: it
    /// matches no `bench-<run-id>` tag.
    #[test]
    fn release_tag_filter_never_matches_a_bench_tag() {
        let wf = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".github/workflows/release.yml");
        let text = std::fs::read_to_string(wf).unwrap();
        let mut patterns = Vec::new();
        let mut in_tags = false;
        for line in text.lines() {
            let t = line.trim();
            if t == "tags:" {
                in_tags = true;
                continue;
            }
            if in_tags {
                if let Some(p) = t.strip_prefix("- ") {
                    patterns.push(p.trim_matches('\'').trim_matches('"').to_string());
                } else {
                    in_tags = false;
                }
            }
        }
        assert_eq!(
            patterns,
            vec!["**[0-9]+.[0-9]+.[0-9]+*".to_string()],
            "release.yml tag filter changed; re-check bench tags"
        );
        for p in &patterns {
            for release in ["v0.5.1", "v0.4.0", "quorum-cli-v0.5.1", "v1.2.3-rc.1"] {
                assert!(
                    github_filter_matches(p, release),
                    "{p} should match {release}"
                );
            }
            let now = time::OffsetDateTime::now_utc();
            for _ in 0..200 {
                let tag = format!("bench-{}", new_run_id(now));
                assert!(!github_filter_matches(p, &tag), "{p} matched {tag}");
            }
            // The filter needs two literal dots between digit runs; a
            // dotted bench tag would match, which is why ids are dot-free.
            assert!(github_filter_matches(p, "bench-2026.11.24"));
        }
    }
}
