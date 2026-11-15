//! Integration tests for the public `name`-table accessors against the
//! DejaVu Sans 2.37 fixture. DejaVu ships a rich `name` table (Macintosh
//! Roman + Windows Unicode records, nameIDs 0..17) so it exercises the
//! well-known accessors, locale-targeted lookup, and record enumeration.
//!
//! Spec: Adobe Technical Note #5149 §1.2 (Platform / Script / Language
//! IDs) and §1.3–1.10 (per-`nameID` semantics).

use oxideav_ttf::{Font, name_id, platform};

const FIXTURE_SANS: &[u8] = include_bytes!("fixtures/DejaVuSans.ttf");

#[test]
fn well_known_name_accessors_resolve_in_dejavu_sans() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();

    assert_eq!(f.family_name(), Some("DejaVu Sans"));
    assert_eq!(f.subfamily_name(), Some("Book"));
    assert_eq!(f.postscript_name(), Some("DejaVuSans"));
    assert_eq!(f.version_string(), Some("Version 2.37"));

    // nameID 16/17 present in DejaVu; preferred-family accessors return
    // those directly.
    assert_eq!(f.typographic_family_name(), Some("DejaVu Sans"));
    assert_eq!(f.typographic_subfamily_name(), Some("Book"));

    let copyright = f.copyright().expect("copyright");
    assert!(
        copyright.contains("Bitstream"),
        "unexpected copyright: {copyright:?}"
    );

    assert_eq!(
        f.license_url(),
        Some("http://dejavu.sourceforge.net/wiki/index.php/License")
    );
    assert_eq!(f.vendor_url(), Some("http://dejavu.sourceforge.net"));
    let license = f.license_description().expect("license");
    assert!(!license.is_empty());
}

#[test]
fn name_string_generic_lookup_matches_typed_accessor() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();
    assert_eq!(f.name_string(name_id::FAMILY), f.family_name());
    assert_eq!(f.name_string(name_id::VERSION), f.version_string());
    // A nameID DejaVu doesn't ship (designer, 9) returns None.
    assert_eq!(f.name_string(name_id::DESIGNER), None);
}

#[test]
fn name_string_for_targets_exact_locale() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();
    // Windows Unicode English (US) family record exists.
    assert_eq!(
        f.name_string_for(name_id::FAMILY, platform::WINDOWS, 0x0409)
            .as_deref(),
        Some("DejaVu Sans")
    );
    // A language DejaVu doesn't carry -> None.
    assert_eq!(
        f.name_string_for(name_id::FAMILY, platform::WINDOWS, 0x0407),
        None
    );
}

#[test]
fn name_records_enumerates_with_locator_tuples() {
    let f = Font::from_bytes(FIXTURE_SANS).unwrap();
    let recs = f.name_records();
    assert!(
        recs.len() >= 13,
        "expected a rich name table, got {}",
        recs.len()
    );

    // DejaVu ships both Macintosh (1) and Windows (3) platform records.
    assert!(recs.iter().any(|r| r.platform_id == platform::MACINTOSH));
    assert!(recs.iter().any(|r| r.platform_id == platform::WINDOWS));

    // The family-name record is present, decodable, and carries the right
    // locator fields.
    let fam = recs
        .iter()
        .find(|r| r.name_id == name_id::FAMILY && r.platform_id == platform::WINDOWS)
        .expect("windows family record");
    assert_eq!(fam.encoding_id, 1); // Unicode BMP
    assert_eq!(fam.string.as_deref(), Some("DejaVu Sans"));

    // Every Windows-Unicode and Mac-Roman record decodes (no None among
    // the encodings we support for this Latin font).
    for r in &recs {
        let supported = (r.platform_id == platform::WINDOWS
            && (r.encoding_id == 1 || r.encoding_id == 10))
            || (r.platform_id == platform::UNICODE)
            || (r.platform_id == platform::MACINTOSH && r.encoding_id == 0);
        if supported {
            assert!(
                r.string.is_some(),
                "expected decodable record p={} e={} n={}",
                r.platform_id,
                r.encoding_id,
                r.name_id
            );
        }
    }
}

/// The name accessors decode a UTF-16 record once per font and hand out
/// the same string on every later call, instead of allocating per call.
#[test]
fn name_accessors_return_one_cached_string_per_name() {
    let f = Font::from_bytes(include_bytes!("fixtures/DejaVuSans.ttf")).unwrap();
    let first = f.family_name().expect("family name");
    assert_eq!(first, "DejaVu Sans");
    for _ in 0..1000 {
        let again = f.family_name().unwrap();
        assert_eq!(again.as_ptr(), first.as_ptr(), "same backing string");
    }
    assert_eq!(f.name_string(oxideav_ttf::name_id::FAMILY), Some(first));
    assert_eq!(
        f.name_string(oxideav_ttf::name_id::FAMILY)
            .unwrap()
            .as_ptr(),
        first.as_ptr()
    );
    // An id the table has no record for.
    assert_eq!(f.name_string(0x7fff), None);
    // Every accessor agrees with the per-record listing's best match.
    let full = f.full_name().unwrap();
    assert!(
        f.name_records()
            .iter()
            .any(|r| r.name_id == oxideav_ttf::name_id::FULL_NAME
                && r.string.as_deref() == Some(full))
    );
}
