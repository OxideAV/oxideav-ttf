//! Byte-level builders for synthetic OpenType Layout tables (GSUB,
//! GPOS, GDEF) used by the shaping conformance tests.
//!
//! Every layout here is transcribed from the table definitions of the
//! OpenType specification chapters "OpenType Layout Common Table
//! Formats", "GSUB", "GPOS", "GDEF" and "OpenType Font Variations
//! Common Table Formats". A fixture font supplies the glyphs, `cmap`
//! and metrics; its layout tables are swapped for ones built here, so a
//! test states the lookup data it exercises in full.

#![allow(dead_code)]

/// Rebuild an sfnt with table `tag`'s body replaced: the new body is
/// appended at EOF and the directory record repointed.
pub fn replace_table(font: &[u8], tag: [u8; 4], body: &[u8]) -> Vec<u8> {
    let num_tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    let mut out = font.to_vec();
    let rec = (0..num_tables)
        .map(|i| 12 + i * 16)
        .find(|&rec| font[rec..rec + 4] == tag)
        .expect("table present");
    // Keep the appended body 4-byte aligned.
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let at = out.len() as u32;
    out[rec + 8..rec + 12].copy_from_slice(&at.to_be_bytes());
    out[rec + 12..rec + 16].copy_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// Rebuild an sfnt with one more table, `tag` (not already present).
/// The table directory is rewritten in tag order and every table body
/// re-laid out, 4-byte aligned; checksums are left as they were.
pub fn add_table(font: &[u8], tag: [u8; 4], body: &[u8]) -> Vec<u8> {
    let num_tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = (0..num_tables)
        .map(|i| {
            let rec = 12 + i * 16;
            let word =
                |at: usize| u32::from_be_bytes(font[at..at + 4].try_into().unwrap()) as usize;
            let (offset, length) = (word(rec + 8), word(rec + 12));
            (
                font[rec..rec + 4].try_into().unwrap(),
                font[offset..offset + length].to_vec(),
            )
        })
        .collect();
    assert!(tables.iter().all(|t| t.0 != tag), "table already present");
    tables.push((tag, body.to_vec()));
    tables.sort_by_key(|t| t.0);

    let mut out = font[..4].to_vec();
    out.extend((tables.len() as u16).to_be_bytes());
    // searchRange, entrySelector, rangeShift.
    let entry_selector = (tables.len() as u16).ilog2() as u16;
    let search_range = (1u16 << entry_selector) * 16;
    out.extend(words(&[
        search_range,
        entry_selector,
        tables.len() as u16 * 16 - search_range,
    ]));
    let mut offset = 12 + tables.len() * 16;
    let mut bodies: Vec<u8> = Vec::new();
    for (table_tag, table) in &tables {
        out.extend(table_tag);
        out.extend([0u8; 4]);
        out.extend((offset as u32).to_be_bytes());
        out.extend((table.len() as u32).to_be_bytes());
        bodies.extend_from_slice(table);
        while !bodies.len().is_multiple_of(4) {
            bodies.push(0);
        }
        offset = 12 + tables.len() * 16 + bodies.len();
    }
    out.extend(bodies);
    out
}

pub fn words(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn sorted(glyphs: &[u16]) -> Vec<u16> {
    let mut sorted = glyphs.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

/// Coverage format 1 (sorted glyph list).
pub fn coverage(glyphs: &[u16]) -> Vec<u8> {
    let sorted = sorted(glyphs);
    let mut b = words(&[1, sorted.len() as u16]);
    b.extend(words(&sorted));
    b
}

/// Coverage format 2 (one range per run of consecutive glyph ids).
pub fn coverage_ranges(glyphs: &[u16]) -> Vec<u8> {
    let sorted = sorted(glyphs);
    let mut ranges: Vec<(u16, u16, u16)> = Vec::new();
    for (index, &g) in sorted.iter().enumerate() {
        match ranges.last_mut() {
            Some(last) if last.1 + 1 == g => last.1 = g,
            _ => ranges.push((g, g, index as u16)),
        }
    }
    let mut b = words(&[2, ranges.len() as u16]);
    for (start, end, start_index) in ranges {
        b.extend(words(&[start, end, start_index]));
    }
    b
}

/// ClassDef format 2: one single-glyph range per `(glyph, class)`.
pub fn class_def(classes: &[(u16, u16)]) -> Vec<u8> {
    let mut sorted = classes.to_vec();
    sorted.sort_unstable();
    let mut b = words(&[2, sorted.len() as u16]);
    for (glyph, class) in sorted {
        b.extend(words(&[glyph, glyph, class]));
    }
    b
}

/// A table made of a fixed header, then offset-addressed parts: the
/// header's `u16` at each `slots[i]` is patched with the offset of
/// `parts[i]` from the start of the table.
pub fn with_parts(mut header: Vec<u8>, slots: &[usize], parts: &[Vec<u8>]) -> Vec<u8> {
    assert_eq!(slots.len(), parts.len());
    for (slot, part) in slots.iter().zip(parts) {
        let at = u16::try_from(header.len()).expect("offset fits");
        header[*slot..*slot + 2].copy_from_slice(&at.to_be_bytes());
        header.extend_from_slice(part);
    }
    header
}

/// `header` followed by an Offset16 array (one slot per part) and the
/// parts themselves; offsets from the start of the table.
fn with_offset_array(mut header: Vec<u8>, parts: &[Vec<u8>]) -> Vec<u8> {
    let first = header.len();
    header.extend(std::iter::repeat_n(0u8, parts.len() * 2));
    let slots: Vec<usize> = (0..parts.len()).map(|i| first + i * 2).collect();
    with_parts(header, &slots, parts)
}

/// One lookup: its type, `lookupFlag`, optional mark filtering set and
/// sub-tables.
pub struct Lookup {
    pub kind: u16,
    pub flags: u16,
    pub mark_set: Option<u16>,
    pub subtables: Vec<Vec<u8>>,
}

impl Lookup {
    pub fn new(kind: u16, subtables: Vec<Vec<u8>>) -> Self {
        Lookup {
            kind,
            flags: 0,
            mark_set: None,
            subtables,
        }
    }

    pub fn flags(mut self, flags: u16) -> Self {
        self.flags = flags;
        self
    }

    /// Sets USE_MARK_FILTERING_SET and the `markFilteringSet` field.
    pub fn mark_set(mut self, set: u16) -> Self {
        self.flags |= 0x0010;
        self.mark_set = Some(set);
        self
    }
}

/// LookupFlag bits.
pub const RIGHT_TO_LEFT: u16 = 0x0001;
pub const IGNORE_BASE_GLYPHS: u16 = 0x0002;
pub const IGNORE_LIGATURES: u16 = 0x0004;
pub const IGNORE_MARKS: u16 = 0x0008;
/// `MARK_ATTACHMENT_CLASS_FILTER` for mark attachment class `class`.
pub const fn mark_attachment_class(class: u16) -> u16 {
    class << 8
}

/// A complete GSUB / GPOS table (version 1.0) with one script, `DFLT`,
/// whose default language system lists `features` — each a tag and the
/// lookups it references. `required` names the index of the feature to
/// store as the language system's required feature instead.
/// `extension == Some(t)` stores every lookup as type `t` with
/// Extension sub-tables pointing at the real ones.
pub fn layout_table_with(
    features: &[([u8; 4], &[u16])],
    required: Option<u16>,
    lookups: &[Lookup],
    extension: Option<u16>,
) -> Vec<u8> {
    // ScriptList → Script → DefaultLangSys.
    let optional: Vec<u16> = (0..features.len() as u16)
        .filter(|&i| Some(i) != required)
        .collect();
    let mut script_list = words(&[1]);
    script_list.extend(b"DFLT");
    script_list.extend(words(&[8])); // Script offset
    script_list.extend(words(&[4, 0])); // defaultLangSys offset, langSysCount
    script_list.extend(words(&[
        0,
        required.unwrap_or(0xFFFF),
        optional.len() as u16,
    ]));
    script_list.extend(words(&optional));

    // FeatureList.
    let mut feature_list = words(&[features.len() as u16]);
    let mut feature_tables: Vec<u8> = Vec::new();
    let records_len = 2 + features.len() * 6;
    for (tag, lookup_indices) in features {
        feature_list.extend(tag);
        feature_list.extend(words(&[(records_len + feature_tables.len()) as u16]));
        feature_tables.extend(words(&[0, lookup_indices.len() as u16]));
        feature_tables.extend(words(lookup_indices));
    }
    feature_list.extend(feature_tables);

    // LookupList.
    let mut lookup_tables: Vec<Vec<u8>> = Vec::new();
    for lookup in lookups {
        let n = lookup.subtables.len();
        let stored_kind = extension.unwrap_or(lookup.kind);
        let mut table = words(&[stored_kind, lookup.flags, n as u16]);
        let header_len = 6 + n * 2 + if lookup.mark_set.is_some() { 2 } else { 0 };
        let mut bodies: Vec<u8> = Vec::new();
        let mut offsets: Vec<u16> = Vec::new();
        match extension {
            None => {
                for sub in &lookup.subtables {
                    offsets.push((header_len + bodies.len()) as u16);
                    bodies.extend_from_slice(sub);
                }
            }
            Some(_) => {
                // n Extension records (8 bytes each), then the targets.
                let mut targets: Vec<u8> = Vec::new();
                for (i, sub) in lookup.subtables.iter().enumerate() {
                    let record_at = header_len + i * 8;
                    offsets.push(record_at as u16);
                    let target_at = header_len + n * 8 + targets.len();
                    bodies.extend(words(&[1, lookup.kind]));
                    bodies.extend(((target_at - record_at) as u32).to_be_bytes());
                    targets.extend_from_slice(sub);
                }
                bodies.extend(targets);
            }
        }
        table.extend(words(&offsets));
        if let Some(set) = lookup.mark_set {
            table.extend(words(&[set]));
        }
        table.extend(bodies);
        lookup_tables.push(table);
    }
    let mut lookup_list = words(&[lookups.len() as u16]);
    let mut at = 2 + lookups.len() * 2;
    for table in &lookup_tables {
        lookup_list.extend(words(&[u16::try_from(at).expect("lookup offset fits")]));
        at += table.len();
    }
    for table in &lookup_tables {
        lookup_list.extend_from_slice(table);
    }

    let script_at = 10usize;
    let feature_at = script_at + script_list.len();
    let lookup_at = feature_at + feature_list.len();
    let mut table = words(&[1, 0, script_at as u16, feature_at as u16, lookup_at as u16]);
    table.extend(script_list);
    table.extend(feature_list);
    table.extend(lookup_list);
    table
}

/// [`layout_table_with`] with no required feature and no Extension
/// wrapping.
pub fn layout_table(features: &[([u8; 4], &[u16])], lookups: &[Lookup]) -> Vec<u8> {
    layout_table_with(features, None, lookups, None)
}

/// A GDEF table (version 1.2, or 1.3 when `item_variation_store` is
/// given): GlyphClassDef from `classes` (`(glyph, class)`; 1 base, 2
/// ligature, 3 mark), MarkAttachClassDef from `mark_attach`, and one
/// mark glyph set per entry of `mark_sets`.
pub fn gdef(
    classes: &[(u16, u16)],
    mark_attach: &[(u16, u16)],
    mark_sets: &[&[u16]],
    item_variation_store: Option<&[u8]>,
) -> Vec<u8> {
    let mut sets = words(&[1, mark_sets.len() as u16]);
    let mut coverages: Vec<u8> = Vec::new();
    let sets_header = 4 + mark_sets.len() * 4;
    for set in mark_sets {
        sets.extend(((sets_header + coverages.len()) as u32).to_be_bytes());
        coverages.extend(coverage(set));
    }
    sets.extend(coverages);

    let minor = if item_variation_store.is_some() { 3 } else { 2 };
    let mut header = words(&[1, minor, 0, 0, 0, 0, 0]);
    if item_variation_store.is_some() {
        header.extend([0u8; 4]);
    }
    let mut table = with_parts(
        header,
        &[4, 10, 12],
        &[class_def(classes), class_def(mark_attach), sets],
    );
    if let Some(store) = item_variation_store {
        let at = table.len() as u32;
        table[14..18].copy_from_slice(&at.to_be_bytes());
        table.extend_from_slice(store);
    }
    table
}

/// An ItemVariationStore with one ItemVariationData sub-table.
/// `regions[r][axis]` is the `(start, peak, end)` of region `r` on each
/// axis, in normalised units; `delta_sets[item][r]` the delta of item
/// `item` for region `r`. A VariationIndex `(outer 0, inner item)`
/// addresses each delta set.
pub fn item_variation_store(regions: &[Vec<(f32, f32, f32)>], delta_sets: &[Vec<i16>]) -> Vec<u8> {
    let axis_count = regions.first().map_or(0, Vec::len);
    let f2dot14 = |v: f32| ((v * 16384.0).round() as i16).to_be_bytes();
    let mut region_list = words(&[axis_count as u16, regions.len() as u16]);
    for region in regions {
        assert_eq!(region.len(), axis_count);
        for &(start, peak, end) in region {
            region_list.extend(f2dot14(start));
            region_list.extend(f2dot14(peak));
            region_list.extend(f2dot14(end));
        }
    }
    // ItemVariationData: every delta stored as a word (int16).
    let mut data = words(&[
        delta_sets.len() as u16,
        regions.len() as u16,
        regions.len() as u16,
    ]);
    data.extend(words(&(0..regions.len() as u16).collect::<Vec<_>>()));
    for set in delta_sets {
        assert_eq!(set.len(), regions.len());
        for &delta in set {
            data.extend(delta.to_be_bytes());
        }
    }
    // Header: format, Offset32 regionList, count, Offset32 data[1].
    let header_len = 2 + 4 + 2 + 4;
    let mut store = words(&[1]);
    store.extend((header_len as u32).to_be_bytes());
    store.extend(words(&[1]));
    store.extend(((header_len + region_list.len()) as u32).to_be_bytes());
    store.extend(region_list);
    store.extend(data);
    store
}

/// A VariationIndex table for delta set `inner` of sub-table `outer`.
pub fn variation_index(outer: u16, inner: u16) -> Vec<u8> {
    words(&[outer, inner, 0x8000])
}

// ---- GSUB sub-tables -------------------------------------------------

/// SingleSubstFormat2: explicit `(glyph, substitute)` pairs.
pub fn single_subst(pairs: &[(u16, u16)]) -> Vec<u8> {
    let mut sorted = pairs.to_vec();
    sorted.sort_unstable();
    let mut header = words(&[2, 0, sorted.len() as u16]);
    header.extend(words(&sorted.iter().map(|p| p.1).collect::<Vec<_>>()));
    let glyphs: Vec<u16> = sorted.iter().map(|p| p.0).collect();
    with_parts(header, &[2], &[coverage(&glyphs)])
}

/// MultipleSubstFormat1: `(glyph, replacement sequence)`.
pub fn multiple_subst(rules: &[(u16, &[u16])]) -> Vec<u8> {
    let mut sorted = rules.to_vec();
    sorted.sort_unstable_by_key(|r| r.0);
    let glyphs: Vec<u16> = sorted.iter().map(|r| r.0).collect();
    let sequences: Vec<Vec<u8>> = sorted
        .iter()
        .map(|(_, sequence)| {
            let mut b = words(&[sequence.len() as u16]);
            b.extend(words(sequence));
            b
        })
        .collect();
    let table = with_offset_array(words(&[1, 0, sorted.len() as u16]), &sequences);
    with_parts(table, &[2], &[coverage(&glyphs)])
}

/// AlternateSubstFormat1: `(glyph, alternates)`.
pub fn alternate_subst(rules: &[(u16, &[u16])]) -> Vec<u8> {
    // AlternateSet has the layout of a MultipleSubst Sequence.
    multiple_subst(rules)
}

/// LigatureSubstFormat1: `(components, ligature)`; ligatures sharing a
/// first component keep the order given.
pub fn ligature_subst(rules: &[(&[u16], u16)]) -> Vec<u8> {
    let firsts = sorted(&rules.iter().map(|r| r.0[0]).collect::<Vec<_>>());
    let sets: Vec<Vec<u8>> = firsts
        .iter()
        .map(|&first| {
            let ligatures: Vec<Vec<u8>> = rules
                .iter()
                .filter(|r| r.0[0] == first)
                .map(|(components, ligature)| {
                    let mut b = words(&[*ligature, components.len() as u16]);
                    b.extend(words(&components[1..]));
                    b
                })
                .collect();
            with_offset_array(words(&[ligatures.len() as u16]), &ligatures)
        })
        .collect();
    let table = with_offset_array(words(&[1, 0, firsts.len() as u16]), &sets);
    with_parts(table, &[2], &[coverage(&firsts)])
}

/// SequenceLookup records `(sequenceIndex, lookupListIndex)`.
fn sequence_lookups(records: &[(u16, u16)]) -> Vec<u8> {
    records.iter().flat_map(|&(s, l)| words(&[s, l])).collect()
}

/// One sequence rule: the input sequence (glyph ids or class values)
/// and its SequenceLookup records.
pub type SeqRule<'r> = (&'r [u16], &'r [(u16, u16)]);

/// SequenceContextFormat1: rules as `(input glyphs, records)`.
pub fn context_glyphs(rules: &[SeqRule<'_>]) -> Vec<u8> {
    let firsts = sorted(&rules.iter().map(|r| r.0[0]).collect::<Vec<_>>());
    let sets: Vec<Vec<u8>> = firsts
        .iter()
        .map(|&first| {
            let set: Vec<Vec<u8>> = rules
                .iter()
                .filter(|r| r.0[0] == first)
                .map(|(input, records)| {
                    let mut b = words(&[input.len() as u16, records.len() as u16]);
                    b.extend(words(&input[1..]));
                    b.extend(sequence_lookups(records));
                    b
                })
                .collect();
            with_offset_array(words(&[set.len() as u16]), &set)
        })
        .collect();
    let table = with_offset_array(words(&[1, 0, firsts.len() as u16]), &sets);
    with_parts(table, &[2], &[coverage(&firsts)])
}

/// SequenceContextFormat2: `classes` is the ClassDef; rules as `(input
/// classes, records)`; `first_glyphs` the Coverage of first glyphs.
pub fn context_classes(
    first_glyphs: &[u16],
    classes: &[(u16, u16)],
    rules: &[SeqRule<'_>],
) -> Vec<u8> {
    let class_count = classes.iter().map(|c| c.1).max().unwrap_or(0) + 1;
    let mut header = words(&[2, 0, 0, class_count]);
    let slots_at = header.len();
    header.extend(std::iter::repeat_n(0u8, class_count as usize * 2));
    let mut slots = vec![2, 4];
    let mut parts = vec![coverage(first_glyphs), class_def(classes)];
    for class in 0..class_count {
        let set: Vec<Vec<u8>> = rules
            .iter()
            .filter(|r| r.0[0] == class)
            .map(|(input, records)| {
                let mut b = words(&[input.len() as u16, records.len() as u16]);
                b.extend(words(&input[1..]));
                b.extend(sequence_lookups(records));
                b
            })
            .collect();
        if !set.is_empty() {
            slots.push(slots_at + class as usize * 2);
            parts.push(with_offset_array(words(&[set.len() as u16]), &set));
        }
    }
    with_parts(header, &slots, &parts)
}

/// SequenceContextFormat3: one Coverage per input glyph.
pub fn context_coverage(input: &[&[u16]], records: &[(u16, u16)]) -> Vec<u8> {
    let mut header = words(&[3, input.len() as u16, records.len() as u16]);
    let slots: Vec<usize> = (0..input.len()).map(|i| 6 + i * 2).collect();
    header.extend(std::iter::repeat_n(0u8, input.len() * 2));
    header.extend(sequence_lookups(records));
    let parts: Vec<Vec<u8>> = input.iter().map(|g| coverage(g)).collect();
    with_parts(header, &slots, &parts)
}

/// One chained rule: backtrack (nearest glyph first), input, lookahead
/// and records.
pub type ChainRule<'r> = (&'r [u16], &'r [u16], &'r [u16], &'r [(u16, u16)]);

fn chain_rule(rule: &ChainRule<'_>) -> Vec<u8> {
    let (backtrack, input, lookahead, records) = rule;
    let mut b = words(&[backtrack.len() as u16]);
    b.extend(words(backtrack));
    b.extend(words(&[input.len() as u16]));
    b.extend(words(&input[1..]));
    b.extend(words(&[lookahead.len() as u16]));
    b.extend(words(lookahead));
    b.extend(words(&[records.len() as u16]));
    b.extend(sequence_lookups(records));
    b
}

/// ChainedSequenceContextFormat1: rules in glyph ids.
pub fn chain_glyphs(rules: &[ChainRule<'_>]) -> Vec<u8> {
    let firsts = sorted(&rules.iter().map(|r| r.1[0]).collect::<Vec<_>>());
    let sets: Vec<Vec<u8>> = firsts
        .iter()
        .map(|&first| {
            let set: Vec<Vec<u8>> = rules
                .iter()
                .filter(|r| r.1[0] == first)
                .map(chain_rule)
                .collect();
            with_offset_array(words(&[set.len() as u16]), &set)
        })
        .collect();
    let table = with_offset_array(words(&[1, 0, firsts.len() as u16]), &sets);
    with_parts(table, &[2], &[coverage(&firsts)])
}

/// ChainedSequenceContextFormat2: one ClassDef shared by the backtrack,
/// input and lookahead sequences; rules in class values.
pub fn chain_classes(
    first_glyphs: &[u16],
    classes: &[(u16, u16)],
    rules: &[ChainRule<'_>],
) -> Vec<u8> {
    let class_count = classes.iter().map(|c| c.1).max().unwrap_or(0) + 1;
    let mut header = words(&[2, 0, 0, 0, 0, class_count]);
    let slots_at = header.len();
    header.extend(std::iter::repeat_n(0u8, class_count as usize * 2));
    let mut slots = vec![2, 4, 6, 8];
    let def = class_def(classes);
    let mut parts = vec![coverage(first_glyphs), def.clone(), def.clone(), def];
    for class in 0..class_count {
        let set: Vec<Vec<u8>> = rules
            .iter()
            .filter(|r| r.1[0] == class)
            .map(chain_rule)
            .collect();
        if !set.is_empty() {
            slots.push(slots_at + class as usize * 2);
            parts.push(with_offset_array(words(&[set.len() as u16]), &set));
        }
    }
    with_parts(header, &slots, &parts)
}

/// ChainedSequenceContextFormat3: a Coverage per backtrack (nearest
/// first), input and lookahead glyph.
pub fn chain_coverage(
    backtrack: &[&[u16]],
    input: &[&[u16]],
    lookahead: &[&[u16]],
    records: &[(u16, u16)],
) -> Vec<u8> {
    let mut header = words(&[3]);
    let mut slots = Vec::new();
    let mut parts = Vec::new();
    for sequence in [backtrack, input, lookahead] {
        header.extend(words(&[sequence.len() as u16]));
        for glyphs in sequence {
            slots.push(header.len());
            header.extend([0u8; 2]);
            parts.push(coverage(glyphs));
        }
    }
    header.extend(words(&[records.len() as u16]));
    header.extend(sequence_lookups(records));
    with_parts(header, &slots, &parts)
}

/// ReverseChainSingleSubstFormat1: `(glyph, substitute)` pairs applied
/// between the backtrack (nearest first) and lookahead Coverages.
pub fn reverse_chain(pairs: &[(u16, u16)], backtrack: &[&[u16]], lookahead: &[&[u16]]) -> Vec<u8> {
    let mut sorted = pairs.to_vec();
    sorted.sort_unstable();
    let mut header = words(&[1, 0]);
    let mut slots = vec![2];
    let mut parts = vec![coverage(&sorted.iter().map(|p| p.0).collect::<Vec<_>>())];
    for sequence in [backtrack, lookahead] {
        header.extend(words(&[sequence.len() as u16]));
        for glyphs in sequence {
            slots.push(header.len());
            header.extend([0u8; 2]);
            parts.push(coverage(glyphs));
        }
    }
    header.extend(words(&[sorted.len() as u16]));
    header.extend(words(&sorted.iter().map(|p| p.1).collect::<Vec<_>>()));
    with_parts(header, &slots, &parts)
}

// ---- GPOS sub-tables -------------------------------------------------

/// ValueFormat bits.
pub const X_PLACEMENT: u16 = 0x0001;
pub const Y_PLACEMENT: u16 = 0x0002;
pub const X_ADVANCE: u16 = 0x0004;
pub const Y_ADVANCE: u16 = 0x0008;
pub const X_PLACEMENT_DEVICE: u16 = 0x0010;
pub const X_ADVANCE_DEVICE: u16 = 0x0040;

/// An anchor: coordinates, plus — for AnchorFormat3 — the delta sets
/// (VariationIndex inner indices, outer 0) varying x and y.
#[derive(Clone, Copy)]
pub struct Anchor {
    pub x: i16,
    pub y: i16,
    pub variation: Option<(u16, u16)>,
}

pub const fn at(x: i16, y: i16) -> Anchor {
    Anchor {
        x,
        y,
        variation: None,
    }
}

fn anchor(a: &Anchor) -> Vec<u8> {
    match a.variation {
        None => words(&[1, a.x as u16, a.y as u16]),
        Some((x_set, y_set)) => with_parts(
            words(&[3, a.x as u16, a.y as u16, 0, 0]),
            &[6, 8],
            &[variation_index(0, x_set), variation_index(0, y_set)],
        ),
    }
}

/// An array of records, each `width` optional anchors; offsets from the
/// start of the array. `count_prefix` writes the leading record count.
fn anchor_matrix(rows: &[Vec<Option<Anchor>>]) -> Vec<u8> {
    let mut header = words(&[rows.len() as u16]);
    let mut slots = Vec::new();
    let mut parts = Vec::new();
    for row in rows {
        for cell in row {
            if let Some(a) = cell {
                slots.push(header.len());
                parts.push(anchor(a));
            }
            header.extend([0u8; 2]);
        }
    }
    with_parts(header, &slots, &parts)
}

/// MarkArray for `marks` (`(glyph, class, anchor)`, already in Coverage
/// order).
fn mark_array(marks: &[(u16, u16, Anchor)]) -> Vec<u8> {
    let mut header = words(&[marks.len() as u16]);
    let mut slots = Vec::new();
    let mut parts = Vec::new();
    for (_, class, a) in marks {
        header.extend(words(&[*class]));
        slots.push(header.len());
        header.extend([0u8; 2]);
        parts.push(anchor(a));
    }
    with_parts(header, &slots, &parts)
}

/// SinglePosFormat1: one ValueRecord (`values`, in ValueFormat field
/// order) for every glyph of `glyphs`.
pub fn single_pos(glyphs: &[u16], value_format: u16, values: &[i16]) -> Vec<u8> {
    let mut header = words(&[1, 0, value_format]);
    header.extend(values.iter().flat_map(|v| v.to_be_bytes()));
    with_parts(header, &[2], &[coverage(glyphs)])
}

/// SinglePosFormat1 whose ValueRecord is `xAdvance` plus an
/// `xAdvance` VariationIndex for delta set `delta_set`.
pub fn single_pos_varied_advance(glyphs: &[u16], x_advance: i16, delta_set: u16) -> Vec<u8> {
    let header = words(&[1, 0, X_ADVANCE | X_ADVANCE_DEVICE, x_advance as u16, 0]);
    with_parts(
        header,
        &[2, 8],
        &[coverage(glyphs), variation_index(0, delta_set)],
    )
}

/// PairPosFormat1: `(first, second, value1, value2)` with the fields of
/// each value in ValueFormat order.
pub fn pair_pos_glyphs(
    format1: u16,
    format2: u16,
    pairs: &[(u16, u16, &[i16], &[i16])],
) -> Vec<u8> {
    let firsts = sorted(&pairs.iter().map(|p| p.0).collect::<Vec<_>>());
    let sets: Vec<Vec<u8>> = firsts
        .iter()
        .map(|&first| {
            let mut records: Vec<_> = pairs.iter().filter(|p| p.0 == first).collect();
            records.sort_unstable_by_key(|p| p.1);
            let mut b = words(&[records.len() as u16]);
            for (_, second, value1, value2) in records {
                assert_eq!(value1.len() as u32, format1.count_ones());
                assert_eq!(value2.len() as u32, format2.count_ones());
                b.extend(words(&[*second]));
                b.extend(value1.iter().flat_map(|v| v.to_be_bytes()));
                b.extend(value2.iter().flat_map(|v| v.to_be_bytes()));
            }
            b
        })
        .collect();
    let table = with_offset_array(words(&[1, 0, format1, format2, firsts.len() as u16]), &sets);
    with_parts(table, &[2], &[coverage(&firsts)])
}

/// PairPosFormat1 with one pair whose first ValueRecord is `xAdvance`
/// plus an `xAdvance` VariationIndex (offset from the PairSet table).
pub fn pair_pos_varied_advance(first: u16, second: u16, x_advance: i16, delta_set: u16) -> Vec<u8> {
    let pair_set = with_parts(
        words(&[1, second, x_advance as u16, 0]),
        &[6],
        &[variation_index(0, delta_set)],
    );
    with_parts(
        words(&[1, 0, X_ADVANCE | X_ADVANCE_DEVICE, 0, 1, 0]),
        &[2, 10],
        &[coverage(&[first]), pair_set],
    )
}

/// `((class1, class2), value1, value2)` of one class pair.
pub type ClassPairValues<'r> = ((u16, u16), &'r [i16], &'r [i16]);

/// PairPosFormat2: class pairs. `first_classes` / `second_classes` are
/// the two ClassDefs; `values[(class1, class2)] = (value1, value2)`,
/// absent class pairs get zero records.
pub fn pair_pos_classes(
    first_glyphs: &[u16],
    format1: u16,
    format2: u16,
    first_classes: &[(u16, u16)],
    second_classes: &[(u16, u16)],
    values: &[ClassPairValues<'_>],
) -> Vec<u8> {
    let class1_count = first_classes.iter().map(|c| c.1).max().unwrap_or(0) + 1;
    let class2_count = second_classes.iter().map(|c| c.1).max().unwrap_or(0) + 1;
    let mut header = words(&[2, 0, format1, format2, 0, 0, class1_count, class2_count]);
    let (size1, size2) = (format1.count_ones() as usize, format2.count_ones() as usize);
    for class1 in 0..class1_count {
        for class2 in 0..class2_count {
            match values.iter().find(|v| v.0 == (class1, class2)) {
                Some((_, value1, value2)) => {
                    assert_eq!((value1.len(), value2.len()), (size1, size2));
                    header.extend(value1.iter().flat_map(|v| v.to_be_bytes()));
                    header.extend(value2.iter().flat_map(|v| v.to_be_bytes()));
                }
                None => header.extend(std::iter::repeat_n(0u8, (size1 + size2) * 2)),
            }
        }
    }
    with_parts(
        header,
        &[2, 8, 10],
        &[
            coverage(first_glyphs),
            class_def(first_classes),
            class_def(second_classes),
        ],
    )
}

/// CursivePosFormat1: `(glyph, entry, exit)`.
pub fn cursive_pos(glyphs: &[(u16, Option<Anchor>, Option<Anchor>)]) -> Vec<u8> {
    let mut sorted = glyphs.to_vec();
    sorted.sort_unstable_by_key(|g| g.0);
    let mut header = words(&[1, 0, sorted.len() as u16]);
    let mut slots = vec![2];
    let mut parts = vec![coverage(&sorted.iter().map(|g| g.0).collect::<Vec<_>>())];
    for (_, entry, exit) in &sorted {
        for point in [entry, exit] {
            if let Some(a) = point {
                slots.push(header.len());
                parts.push(anchor(a));
            }
            header.extend([0u8; 2]);
        }
    }
    with_parts(header, &slots, &parts)
}

/// MarkBasePosFormat1 (also the layout of MarkMarkPosFormat1): `marks`
/// as `(glyph, class, anchor)`, `bases` as `(glyph, anchor per class)`.
pub fn mark_base_pos(marks: &[(u16, u16, Anchor)], bases: &[(u16, &[Option<Anchor>])]) -> Vec<u8> {
    let mut marks = marks.to_vec();
    marks.sort_unstable_by_key(|m| m.0);
    let mut bases = bases.to_vec();
    bases.sort_unstable_by_key(|b| b.0);
    let class_count = marks.iter().map(|m| m.1).max().unwrap_or(0) + 1;
    let rows: Vec<Vec<Option<Anchor>>> = bases
        .iter()
        .map(|(_, anchors)| {
            assert_eq!(anchors.len(), class_count as usize);
            anchors.to_vec()
        })
        .collect();
    with_parts(
        words(&[1, 0, 0, class_count, 0, 0]),
        &[2, 4, 8, 10],
        &[
            coverage(&marks.iter().map(|m| m.0).collect::<Vec<_>>()),
            coverage(&bases.iter().map(|b| b.0).collect::<Vec<_>>()),
            mark_array(&marks),
            anchor_matrix(&rows),
        ],
    )
}

/// A ligature glyph and, per component, its anchor for each mark class.
pub type LigatureAnchors<'r> = (u16, &'r [&'r [Option<Anchor>]]);

/// MarkLigPosFormat1: `ligatures` as `(glyph, components)`, each
/// component an anchor per mark class.
pub fn mark_ligature_pos(
    marks: &[(u16, u16, Anchor)],
    ligatures: &[LigatureAnchors<'_>],
) -> Vec<u8> {
    let mut marks = marks.to_vec();
    marks.sort_unstable_by_key(|m| m.0);
    let mut ligatures = ligatures.to_vec();
    ligatures.sort_unstable_by_key(|l| l.0);
    let class_count = marks.iter().map(|m| m.1).max().unwrap_or(0) + 1;
    let attach: Vec<Vec<u8>> = ligatures
        .iter()
        .map(|(_, components)| {
            let rows: Vec<Vec<Option<Anchor>>> = components
                .iter()
                .map(|anchors| {
                    assert_eq!(anchors.len(), class_count as usize);
                    anchors.to_vec()
                })
                .collect();
            anchor_matrix(&rows)
        })
        .collect();
    let ligature_array = with_offset_array(words(&[attach.len() as u16]), &attach);
    with_parts(
        words(&[1, 0, 0, class_count, 0, 0]),
        &[2, 4, 8, 10],
        &[
            coverage(&marks.iter().map(|m| m.0).collect::<Vec<_>>()),
            coverage(&ligatures.iter().map(|l| l.0).collect::<Vec<_>>()),
            mark_array(&marks),
            ligature_array,
        ],
    )
}
