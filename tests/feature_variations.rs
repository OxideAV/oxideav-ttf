//! End-to-end coverage for the GSUB `FeatureVariations` substructure
//! (ISO/IEC 14496-22:2019 §6.2.9) through the `Font` boundary.
//!
//! A version-1.1 GSUB header carries an `Offset32 featureVariationsOffset`
//! after the three v1.0 offsets. The table lets a variable font swap the
//! lookups behind a feature for an alternate set when the current
//! variation instance falls inside a normalised range on an `fvar` axis.
//!
//! This suite builds a minimal variable font with one `wght` axis and a
//! GSUB table whose `liga` feature (index 0) resolves to lookup [0] by
//! default and to lookup [1] when the instance's normalised `wght`
//! coordinate is in `[0.5, 1.0]`, then asserts:
//!
//! * the three bundled static fixtures report no feature variations and
//!   the instance-aware accessor matches the plain one;
//! * at the default instance (wght = 400) the default lookup is used;
//! * at wght = 900 (normalised 1.0) the alternate lookup is used.

use oxideav_ttf::Font;

/// Build the smallest sfnt the directory parser accepts.
fn build_minimal_sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&0x00010000u32.to_be_bytes()); // version (TrueType)
    out.extend_from_slice(&n.to_be_bytes()); // numTables
    out.extend_from_slice(&0u16.to_be_bytes()); // searchRange
    out.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
    out.extend_from_slice(&0u16.to_be_bytes()); // rangeShift

    let dir_size = 16usize * n as usize;
    let mut payload_offset = 12usize + dir_size;
    let mut records = Vec::with_capacity(n as usize);
    for (tag, payload) in tables {
        records.push((*tag, payload_offset as u32, payload.len() as u32));
        payload_offset += payload.len();
        while payload_offset % 4 != 0 {
            payload_offset += 1;
        }
    }
    for (tag, offset, length) in &records {
        out.extend_from_slice(tag.as_slice());
        out.extend_from_slice(&0u32.to_be_bytes()); // checksum
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&length.to_be_bytes());
    }
    for (_tag, payload) in tables {
        out.extend_from_slice(payload);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

fn make_head(units_per_em: u16) -> Vec<u8> {
    let mut h = vec![0u8; 54];
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[4..8].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[12..16].copy_from_slice(&0x5F0F3CF5u32.to_be_bytes());
    h[18..20].copy_from_slice(&units_per_em.to_be_bytes());
    h[50..52].copy_from_slice(&0i16.to_be_bytes()); // indexToLocFormat = short
    h
}

fn make_hhea(num_h_metrics: u16) -> Vec<u8> {
    let mut h = vec![0u8; 36];
    h[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
    h[34..36].copy_from_slice(&num_h_metrics.to_be_bytes());
    h
}

fn make_maxp(num_glyphs: u16) -> Vec<u8> {
    let mut h = vec![0u8; 6];
    h[0..4].copy_from_slice(&0x00005000u32.to_be_bytes()); // version 0.5
    h[4..6].copy_from_slice(&num_glyphs.to_be_bytes());
    h
}

fn make_cmap_empty() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes()); // version
    out.extend_from_slice(&1u16.to_be_bytes()); // numTables
    out.extend_from_slice(&3u16.to_be_bytes()); // platformID
    out.extend_from_slice(&1u16.to_be_bytes()); // encodingID
    out.extend_from_slice(&12u32.to_be_bytes()); // offset to subtable

    let mut sub = Vec::new();
    sub.extend_from_slice(&4u16.to_be_bytes()); // format
    sub.extend_from_slice(&0u16.to_be_bytes()); // length (placeholder)
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&2u16.to_be_bytes()); // segCountX2
    sub.extend_from_slice(&2u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes()); // endCode[0]
    sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes()); // startCode[0]
    sub.extend_from_slice(&1u16.to_be_bytes()); // idDelta[0]
    sub.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset[0]
    let total = sub.len() as u16;
    sub[2..4].copy_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&sub);
    out
}

fn make_name_empty() -> Vec<u8> {
    let mut out = vec![0u8; 6];
    out[4..6].copy_from_slice(&6u16.to_be_bytes()); // storageOffset
    out
}

fn make_hmtx(num_h_metrics: u16, num_glyphs: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..num_h_metrics {
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0i16.to_be_bytes());
    }
    for _ in num_h_metrics..num_glyphs {
        out.extend_from_slice(&0i16.to_be_bytes());
    }
    out
}

fn make_loca_short(num_glyphs: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..=num_glyphs {
        out.extend_from_slice(&0u16.to_be_bytes());
    }
    out
}

fn make_glyf_empty() -> Vec<u8> {
    vec![0u8; 2]
}

/// Build an `fvar` table with a single `wght` axis (min/default/max in
/// user space). No named instances.
fn make_fvar_wght(min: f32, default: f32, max: f32) -> Vec<u8> {
    let fixed = |v: f32| -> [u8; 4] { ((v * 65536.0).round() as i32).to_be_bytes() };
    let mut out = Vec::new();
    out.extend_from_slice(&1u16.to_be_bytes()); // majorVersion
    out.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
    out.extend_from_slice(&16u16.to_be_bytes()); // axesArrayOffset
    out.extend_from_slice(&2u16.to_be_bytes()); // reserved
    out.extend_from_slice(&1u16.to_be_bytes()); // axisCount
    out.extend_from_slice(&20u16.to_be_bytes()); // axisSize
    out.extend_from_slice(&0u16.to_be_bytes()); // instanceCount
    out.extend_from_slice(&8u16.to_be_bytes()); // instanceSize (4 + 4*axisCount)
                                                // axis record (20 bytes)
    out.extend_from_slice(b"wght"); // axisTag
    out.extend_from_slice(&fixed(min));
    out.extend_from_slice(&fixed(default));
    out.extend_from_slice(&fixed(max));
    out.extend_from_slice(&0u16.to_be_bytes()); // flags
    out.extend_from_slice(&256u16.to_be_bytes()); // axisNameID
    out
}

/// Build a GSUB v1.1 table:
///   ScriptList: one script `DFLT` with a DefaultLangSys listing one
///     feature index (0).
///   FeatureList: one feature `liga` → default lookup-index list
///     `default_lookups`.
///   LookupList: two no-op single-subst lookups (format-2 with zero
///     glyphs) so the lookup indices the test asserts are valid indices.
///   FeatureVariations: one record whose condition set requires
///     wght-axis (index 0) normalised value in `[min, max]`, substituting
///     feature index 0 with an alternate feature carrying `alt_lookups`.
fn make_gsub_v11(
    default_lookups: &[u16],
    cond_min: i16,
    cond_max: i16,
    alt_lookups: &[u16],
) -> Vec<u8> {
    // --- ScriptList ---
    // u16 scriptCount; ScriptRecord{ Tag, Offset16 } [1]
    // Script{ Offset16 defaultLangSys; u16 langSysCount } @ scriptRel
    // LangSys{ Offset16 lookupOrder=0; u16 required=0xFFFF;
    //          u16 featCount=1; u16 featIndices[0]=0 } @ langSysRel
    let mut sl = Vec::new();
    sl.extend_from_slice(&1u16.to_be_bytes()); // scriptCount
    sl.extend_from_slice(b"DFLT"); // tag
    sl.extend_from_slice(&8u16.to_be_bytes()); // scriptOffset (rel ScriptList)
                                               // Script @ 8
    sl.extend_from_slice(&4u16.to_be_bytes()); // defaultLangSysOffset (rel Script)
    sl.extend_from_slice(&0u16.to_be_bytes()); // langSysCount
                                               // LangSys @ Script+4 = 12
    sl.extend_from_slice(&0u16.to_be_bytes()); // lookupOrderOffset
    sl.extend_from_slice(&0xFFFFu16.to_be_bytes()); // requiredFeatureIndex
    sl.extend_from_slice(&1u16.to_be_bytes()); // featureIndexCount
    sl.extend_from_slice(&0u16.to_be_bytes()); // featureIndices[0] = 0

    // --- FeatureList ---
    // u16 featureCount=1; FeatureRecord{ Tag liga, Offset16 } [1]
    // Feature{ Offset16 params=0; u16 lookupCount; u16 lookups[] }
    let mut fl = Vec::new();
    fl.extend_from_slice(&1u16.to_be_bytes()); // featureCount
    fl.extend_from_slice(b"liga"); // tag
    fl.extend_from_slice(&8u16.to_be_bytes()); // featureOffset (rel FeatureList)
                                               // Feature @ 8
    fl.extend_from_slice(&0u16.to_be_bytes()); // featureParamsOffset
    fl.extend_from_slice(&(default_lookups.len() as u16).to_be_bytes());
    for &li in default_lookups {
        fl.extend_from_slice(&li.to_be_bytes());
    }

    // --- LookupList ---
    // u16 lookupCount; Offset16 lookupOffsets[]; Lookup tables.
    // Each lookup: u16 type=1; u16 flag=0; u16 subTableCount=1;
    //   Offset16 subtable; SingleSubstFormat2{ u16 fmt=2; Offset16 cov;
    //   u16 glyphCount=0 } with an empty Coverage format 1 (count 0).
    let lookup_count = 2u16;
    let mut ll = Vec::new();
    ll.extend_from_slice(&lookup_count.to_be_bytes());
    // Reserve offset slots; patch after laying out lookups.
    let off_slots = ll.len();
    for _ in 0..lookup_count {
        ll.extend_from_slice(&0u16.to_be_bytes());
    }
    let mut lookup_offsets = Vec::new();
    for _ in 0..lookup_count {
        let here = ll.len() as u16;
        lookup_offsets.push(here);
        // Lookup header
        ll.extend_from_slice(&1u16.to_be_bytes()); // lookupType = single subst
        ll.extend_from_slice(&0u16.to_be_bytes()); // lookupFlag
        ll.extend_from_slice(&1u16.to_be_bytes()); // subTableCount
        ll.extend_from_slice(&8u16.to_be_bytes()); // subtableOffset (rel Lookup)
                                                   // SingleSubstFormat2 @ Lookup+8
        ll.extend_from_slice(&2u16.to_be_bytes()); // substFormat
        ll.extend_from_slice(&8u16.to_be_bytes()); // coverageOffset (rel subtable)
        ll.extend_from_slice(&0u16.to_be_bytes()); // glyphCount = 0
                                                   // Coverage format 1 @ subtable+8
        ll.extend_from_slice(&1u16.to_be_bytes()); // coverageFormat
        ll.extend_from_slice(&0u16.to_be_bytes()); // glyphCount = 0
    }
    for (i, off) in lookup_offsets.iter().enumerate() {
        let p = off_slots + i * 2;
        ll[p..p + 2].copy_from_slice(&off.to_be_bytes());
    }

    // --- FeatureVariations ---
    // header (8) + 1 record (8); ConditionSet (1 condition, format 1);
    // FeatureTableSubstitution (1 record → alternate feature).
    let mut fv = Vec::new();
    fv.extend_from_slice(&1u16.to_be_bytes()); // major
    fv.extend_from_slice(&0u16.to_be_bytes()); // minor
    fv.extend_from_slice(&1u32.to_be_bytes()); // recordCount
    let rec = fv.len();
    fv.extend_from_slice(&0u32.to_be_bytes()); // conditionSetOffset (patch)
    fv.extend_from_slice(&0u32.to_be_bytes()); // featureTableSubstOffset (patch)
                                               // ConditionSet
    let cs = fv.len() as u32;
    fv.extend_from_slice(&1u16.to_be_bytes()); // conditionCount
    let cond_off_pos = fv.len();
    fv.extend_from_slice(&0u32.to_be_bytes()); // condition offset (rel cs, patch)
    let cond_rel = fv.len() as u32 - cs;
    fv.extend_from_slice(&1u16.to_be_bytes()); // format 1
    fv.extend_from_slice(&0u16.to_be_bytes()); // axisIndex 0 (wght)
    fv.extend_from_slice(&cond_min.to_be_bytes()); // filterRangeMin
    fv.extend_from_slice(&cond_max.to_be_bytes()); // filterRangeMax
                                                   // FeatureTableSubstitution
    let ss = fv.len() as u32;
    fv.extend_from_slice(&1u16.to_be_bytes()); // major
    fv.extend_from_slice(&0u16.to_be_bytes()); // minor
    fv.extend_from_slice(&1u16.to_be_bytes()); // substitutionCount
    fv.extend_from_slice(&0u16.to_be_bytes()); // featureIndex 0
    let alt_off_pos = fv.len();
    fv.extend_from_slice(&0u32.to_be_bytes()); // altFeatureOffset (rel ss, patch)
    let alt_rel = fv.len() as u32 - ss;
    fv.extend_from_slice(&0u16.to_be_bytes()); // featureParamsOffset
    fv.extend_from_slice(&(alt_lookups.len() as u16).to_be_bytes());
    for &li in alt_lookups {
        fv.extend_from_slice(&li.to_be_bytes());
    }
    fv[rec..rec + 4].copy_from_slice(&cs.to_be_bytes());
    fv[rec + 4..rec + 8].copy_from_slice(&ss.to_be_bytes());
    fv[cond_off_pos..cond_off_pos + 4].copy_from_slice(&cond_rel.to_be_bytes());
    fv[alt_off_pos..alt_off_pos + 4].copy_from_slice(&alt_rel.to_be_bytes());

    // --- Assemble GSUB v1.1 ---
    // Header: major=1, minor=1, scriptList16, featureList16, lookupList16,
    //         featureVariations32.
    let header_len = 14u32;
    let sl_off = header_len;
    let fl_off = sl_off + sl.len() as u32;
    let ll_off = fl_off + fl.len() as u32;
    let fv_off = ll_off + ll.len() as u32;

    let mut gsub = Vec::new();
    gsub.extend_from_slice(&1u16.to_be_bytes()); // majorVersion
    gsub.extend_from_slice(&1u16.to_be_bytes()); // minorVersion = 1
    gsub.extend_from_slice(&(sl_off as u16).to_be_bytes());
    gsub.extend_from_slice(&(fl_off as u16).to_be_bytes());
    gsub.extend_from_slice(&(ll_off as u16).to_be_bytes());
    gsub.extend_from_slice(&fv_off.to_be_bytes());
    gsub.extend_from_slice(&sl);
    gsub.extend_from_slice(&fl);
    gsub.extend_from_slice(&ll);
    gsub.extend_from_slice(&fv);
    gsub
}

fn build_variable_font_with_gsub_fv() -> Vec<u8> {
    let num_glyphs = 4u16;
    // wght axis 400..900, condition matches normalised [0.5, 1.0].
    // F2DOT14(0.5) = 8192, F2DOT14(1.0) = 16384.
    let gsub = make_gsub_v11(&[0], 8192, 16384, &[1]);
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"head", make_head(1000)),
        (b"hhea", make_hhea(1)),
        (b"maxp", make_maxp(num_glyphs)),
        (b"cmap", make_cmap_empty()),
        (b"name", make_name_empty()),
        (b"hmtx", make_hmtx(1, num_glyphs)),
        (b"loca", make_loca_short(num_glyphs)),
        (b"glyf", make_glyf_empty()),
        (b"fvar", make_fvar_wght(400.0, 400.0, 900.0)),
        (b"GSUB", gsub),
    ];
    build_minimal_sfnt(&tables)
}

#[test]
fn synth_variable_font_swaps_lookups_at_high_weight() {
    let bytes = build_variable_font_with_gsub_fv();
    let mut font = Font::from_bytes(&bytes).expect("synth variable font parses");

    assert!(font.is_variable(), "fvar present");
    assert!(
        font.gsub_has_feature_variations(),
        "v1.1 GSUB header carries a FeatureVariations table"
    );

    // At the default instance (wght = 400 → normalised 0.0) the condition
    // [0.5, 1.0] does not match: the default lookup [0] is used.
    let feats = font.gsub_features_for_script_at_instance(*b"DFLT", None);
    assert_eq!(feats.len(), 1);
    assert_eq!(&feats[0].tag, b"liga");
    assert_eq!(
        feats[0].lookup_indices,
        vec![0],
        "default instance keeps the default lookup list"
    );

    // Plain accessor (no variation) always uses the default list.
    let plain = font.gsub_features_for_script(*b"DFLT", None);
    assert_eq!(plain[0].lookup_indices, vec![0]);

    // Move to wght = 900 (normalised 1.0): the condition matches and the
    // alternate feature's lookup list [1] is substituted.
    let mut coords = font.variation_coords().to_vec();
    coords[0] = 900.0;
    font.set_variation_coords(&coords);

    let feats_bold = font.gsub_features_for_script_at_instance(*b"DFLT", None);
    assert_eq!(feats_bold.len(), 1);
    assert_eq!(
        &feats_bold[0].tag, b"liga",
        "feature tag unchanged (§6.2.9)"
    );
    assert_eq!(
        feats_bold[0].lookup_indices,
        vec![1],
        "high-weight instance substitutes the alternate lookup list"
    );

    // The plain accessor still returns the default list — it ignores
    // FeatureVariations entirely.
    let plain_bold = font.gsub_features_for_script(*b"DFLT", None);
    assert_eq!(plain_bold[0].lookup_indices, vec![0]);
}

#[test]
fn bundled_fixtures_report_no_feature_variations() {
    for path in [
        "tests/fixtures/DejaVuSansMono.ttf",
        "tests/fixtures/InterVariable.ttf",
        "tests/fixtures/NotoSansArabic-Regular.ttf",
    ] {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => continue, // fixture not present in this checkout
        };
        let font = Font::from_bytes(&bytes).expect("fixture parses");
        // None of the bundled fonts ship a GSUB FeatureVariations table,
        // so the instance-aware accessor must match the plain one for
        // every script the font lists.
        assert!(
            !font.gsub_has_feature_variations(),
            "{path} unexpectedly reports GSUB feature variations"
        );
        let plain = font.gsub_features_for_script(*b"latn", None);
        let instanced = font.gsub_features_for_script_at_instance(*b"latn", None);
        assert_eq!(
            plain, instanced,
            "{path}: instance-aware accessor must equal the plain one with no feature variations"
        );
    }
}
