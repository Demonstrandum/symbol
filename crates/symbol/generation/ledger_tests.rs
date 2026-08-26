use super::ApiVersion;

#[test]
fn semantic_version_helpers_reset_lower_components() {
    let version = ApiVersion::new(3, 7, 11);
    assert_eq!(version.next_patch(), ApiVersion::new(3, 7, 12));
    assert_eq!(version.next_minor(), ApiVersion::new(3, 8, 0));
    assert_eq!(version.next_major(), ApiVersion::new(4, 0, 0));
}

#[test]
fn semantic_version_parser_is_strict() {
    assert_eq!(
        "1.2.3".parse::<ApiVersion>().unwrap(),
        ApiVersion::new(1, 2, 3)
    );
    for invalid in ["1.2", "1.2.3.4", "01.2.3", "1.02.3", "1.2.03", "1.2.x"] {
        assert!(invalid.parse::<ApiVersion>().is_err(), "{invalid}");
    }
}
