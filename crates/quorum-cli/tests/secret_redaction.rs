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

// ----- Phase 1B Stage 4: env-var-sourced secrets (P11/P31) -----

#[test]
fn session_env_var_value_wrapped_in_secret_redacts() {
    // AC 129: QUORUM_LIPPA_SESSION wrapped in Secret at env read.
    // We can't drive the env-var reading code from here without
    // running the binary, but we CAN verify the type contract: any
    // Secret-wrapped value has the redacting Debug/Display, and the
    // CLI's env-var capture wraps at the read site (see
    // `commands::review::run`'s `QUORUM_LIPPA_SESSION` branch).
    let token = "eyJ1c2VyX2lkIjoxfQ.SOMESIG.LONG-COOKIE-VALUE";
    let wrapped = Secret::new(token.to_string());
    let dbg = format!("{wrapped:?}");
    let disp = format!("{wrapped}");
    assert!(!dbg.contains(token));
    assert!(!disp.contains(token));
}

#[test]
fn password_env_var_value_wrapped_in_secret_redacts() {
    // AC 128: QUORUM_LIPPA_PASSWORD consumed exactly once and dropped.
    // The wrapping discipline is identical to SESSION (the actual drop
    // happens in `commands::auth::login_non_interactive`, after
    // `login_with_cookie` returns). Here we pin the redaction contract
    // — anything that lives outside `expose()` must never print.
    let pw = "hunter2-very-long-and-uniq";
    let wrapped = Secret::new(pw.to_string());
    assert!(!format!("{wrapped:?}").contains(pw));
    assert!(!format!("{wrapped}").contains(pw));
}

#[test]
fn expose_returns_the_inner_string_intact() {
    // Sanity: redaction only hits Display/Debug. expose() is the
    // explicit unwrap that the caller uses exactly once at the API
    // boundary (e.g. `password: password.expose()` into
    // `LoginRequest`).
    let v = "preserved-payload";
    let s = Secret::new(v.into());
    assert_eq!(s.expose(), v);
}
