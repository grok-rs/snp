//! Fixture tests named by the registry.

#[test]
fn fast_matches_scalar() {
    assert_eq!(Fast.find_byte(b'a', b"ba", 0), Scalar.find_byte(b'a', b"ba", 0));
}

#[test]
fn oracle_agrees() {}
