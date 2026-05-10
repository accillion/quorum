//! Secret redaction: Debug/Display must never print the inner value.

use quorum_lippa_client::Secret;

#[test]
fn debug_emits_redacted_marker() {
    let s = Secret::new("hunter2-XYZ-very-secret".into());
    let d = format!("{s:?}");
    assert_eq!(d, "Secret(<redacted>)");
    assert!(!d.contains("hunter2"));
    assert!(!d.contains("XYZ"));
}

#[test]
fn display_emits_redacted_marker() {
    let s = Secret::new("ABCDE".into());
    assert_eq!(format!("{s}"), "<redacted>");
    assert!(!format!("{s}").contains("ABCDE"));
}

#[test]
fn debug_of_option_secret_redacts() {
    let opt: Option<Secret> = Some(Secret::new("hidden-token".into()));
    let dbg = format!("{opt:?}");
    assert!(
        !dbg.contains("hidden-token"),
        "Option<Secret> Debug must propagate redaction"
    );
}

#[test]
fn debug_of_vec_secret_redacts() {
    let v = vec![
        Secret::new("a-leakable".into()),
        Secret::new("b-leakable".into()),
    ];
    let dbg = format!("{v:?}");
    assert!(!dbg.contains("a-leakable"));
    assert!(!dbg.contains("b-leakable"));
}
