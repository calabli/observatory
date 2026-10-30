use observatory_usage_core::{database::Database, lan_usage::pairing};

#[test]
fn code_is_single_use_and_revocation_removes_access() {
    let db = Database::memory().unwrap();
    let (code, _) = pairing::generate(&db).unwrap();
    assert_eq!(code.len(), 6);
    let wrong = if code == "999999" { "999998" } else { "999999" };
    assert!(pairing::accept(&db, wrong, "other").is_err());
    let token = pairing::accept(&db, &code, "peer").unwrap();
    assert!(pairing::authorized(&db, "peer", &token).unwrap());
    assert!(!pairing::authorized(&db, "other", &token).unwrap());
    assert!(pairing::accept(&db, &code, "second").is_err());
    pairing::revoke(&db, "peer").unwrap();
    assert!(!pairing::authorized(&db, "peer", &token).unwrap());
}

#[test]
fn replaced_code_and_excessive_attempts_fail() {
    let db = Database::memory().unwrap();
    let (code, _) = pairing::generate(&db).unwrap();
    let wrong = if code == "000000" { "000001" } else { "000000" };
    for _ in 0..5 {
        assert!(pairing::accept(&db, wrong, "peer").is_err());
    }
    assert!(pairing::accept(&db, &code, "peer").is_err());
}
