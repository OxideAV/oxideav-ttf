//! `JSTF` — the justification table (ISO/IEC 14496-22:2019 §6.3.5).
//!
//! The JSTF table lets a text-processing client justify a line by
//! enabling or disabling specific GSUB / GPOS lookups (and applying
//! dedicated justification lookups) per script and language system. It is
//! organised exactly like GSUB / GPOS: a script list whose records point
//! at per-script data, each carrying a default language system plus
//! optional per-language overrides, and a prioritised list of suggestions.
//!
//! ## Structure (§6.3.5.2)
//!
//! ```text
//!   JSTF header ──> JstfScriptRecord[]  (one per script)
//!                     └─ JstfScript ──> ExtenderGlyph (kashidas, …)
//!                                   ──> default JstfLangSys
//!                                   ──> JstfLangSysRecord[] (per language)
//!                                          └─ JstfLangSys ──> JstfPriority[]
//!                                                               (10 offsets:
//!                                                                shrink/extend
//!                                                                × enable/disable
//!                                                                × GSUB/GPOS
//!                                                                + JstfMax)
//! ```
//!
//! This module decodes the navigational structure (script → language →
//! priority) and the leaf [`JstfPriority`] offset block plus the extender
//! glyph list and the per-priority `Jstf{GSUB,GPOS}ModList` lookup-index
//! arrays. The actual GSUB/GPOS lookups the mod-lists reference live in
//! those tables; JSTF only carries indices into their lookup lists.

use crate::parser::read_u16;
use crate::Error;

/// The 4-byte table tag.
pub const JSTF_TABLE_TAG: [u8; 4] = *b"JSTF";

/// Parsed `JSTF` table: the header plus validated per-script offsets.
#[derive(Debug, Clone)]
pub struct JstfTable<'a> {
    data: &'a [u8],
    /// `(scriptTag, scriptOffset)` from the JSTF header, in file order.
    scripts: Vec<([u8; 4], usize)>,
}

impl<'a> JstfTable<'a> {
    /// Parse the JSTF header (§6.3.5.2) and the script-record list.
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        let major = read_u16(data, 0)?;
        if major != 1 {
            return Err(Error::BadStructure("JSTF major version not 1"));
        }
        let count = read_u16(data, 4)? as usize;
        let mut scripts = Vec::with_capacity(count);
        for i in 0..count {
            let rec = 6 + i * 6; // Tag(4) + Offset16(2)
            let tag = [
                *data.get(rec).ok_or(Error::UnexpectedEof)?,
                *data.get(rec + 1).ok_or(Error::UnexpectedEof)?,
                *data.get(rec + 2).ok_or(Error::UnexpectedEof)?,
                *data.get(rec + 3).ok_or(Error::UnexpectedEof)?,
            ];
            let off = read_u16(data, rec + 4)? as usize;
            if off == 0 || off >= data.len() {
                return Err(Error::BadStructure("JSTF script offset OOB"));
            }
            scripts.push((tag, off));
        }
        Ok(Self { data, scripts })
    }

    /// The script tags present in the table, in file order.
    pub fn script_tags(&self) -> impl Iterator<Item = [u8; 4]> + '_ {
        self.scripts.iter().map(|(t, _)| *t)
    }

    /// Number of scripts.
    pub fn script_count(&self) -> usize {
        self.scripts.len()
    }

    /// Borrow the [`JstfScript`] for `tag`, when present.
    pub fn script(&self, tag: &[u8; 4]) -> Option<JstfScript<'a>> {
        let (_, off) = self.scripts.iter().find(|(t, _)| t == tag)?;
        Some(JstfScript {
            data: self.data,
            base: *off,
        })
    }
}

/// Per-script justification data (§6.3.5.2 JstfScript table).
#[derive(Debug, Clone, Copy)]
pub struct JstfScript<'a> {
    data: &'a [u8],
    base: usize,
}

impl<'a> JstfScript<'a> {
    /// The extender glyph IDs (e.g. Arabic kashidas) for this script, in
    /// increasing numerical order. Empty when the script defines none
    /// (NULL ExtenderGlyph offset).
    pub fn extender_glyphs(&self) -> Vec<u16> {
        let mut out = Vec::new();
        let Ok(off) = read_u16(self.data, self.base) else {
            return out;
        };
        if off == 0 {
            return out;
        }
        let at = self.base + off as usize;
        let Ok(count) = read_u16(self.data, at) else {
            return out;
        };
        for i in 0..count as usize {
            match read_u16(self.data, at + 2 + i * 2) {
                Ok(g) => out.push(g),
                Err(_) => break,
            }
        }
        out
    }

    /// The default [`JstfLangSys`], used when no language-specific record
    /// applies (NULL when the script has no default).
    pub fn default_lang_sys(&self) -> Option<JstfLangSys<'a>> {
        let off = read_u16(self.data, self.base + 2).ok()? as usize;
        if off == 0 {
            return None;
        }
        Some(JstfLangSys {
            data: self.data,
            base: self.base + off,
        })
    }

    /// Number of explicit language-system records.
    pub fn lang_sys_count(&self) -> usize {
        read_u16(self.data, self.base + 4)
            .map(|n| n as usize)
            .unwrap_or(0)
    }

    /// The language-system tags with explicit justification data.
    pub fn lang_sys_tags(&self) -> Vec<[u8; 4]> {
        let n = self.lang_sys_count();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let rec = self.base + 6 + i * 6;
            if let Some(s) = self.data.get(rec..rec + 4) {
                out.push([s[0], s[1], s[2], s[3]]);
            }
        }
        out
    }

    /// Borrow the [`JstfLangSys`] for `tag`, when present.
    pub fn lang_sys(&self, tag: &[u8; 4]) -> Option<JstfLangSys<'a>> {
        let n = self.lang_sys_count();
        for i in 0..n {
            let rec = self.base + 6 + i * 6;
            let t = self.data.get(rec..rec + 4)?;
            if t == tag {
                let off = read_u16(self.data, rec + 4).ok()? as usize;
                if off == 0 {
                    return None;
                }
                return Some(JstfLangSys {
                    data: self.data,
                    base: self.base + off,
                });
            }
        }
        None
    }
}

/// A justification language system: a priority-ordered list of
/// suggestions (§6.3.5.2 JstfLangSys table).
#[derive(Debug, Clone, Copy)]
pub struct JstfLangSys<'a> {
    data: &'a [u8],
    base: usize,
}

impl<'a> JstfLangSys<'a> {
    /// Number of justification-priority levels.
    pub fn priority_count(&self) -> usize {
        read_u16(self.data, self.base)
            .map(|n| n as usize)
            .unwrap_or(0)
    }

    /// Borrow the [`JstfPriority`] at level `index` (0 = highest
    /// priority / "least bad"), or `None` when out of range.
    pub fn priority(&self, index: usize) -> Option<JstfPriority<'a>> {
        if index >= self.priority_count() {
            return None;
        }
        let off = read_u16(self.data, self.base + 2 + index * 2).ok()? as usize;
        if off == 0 {
            return None;
        }
        Some(JstfPriority {
            data: self.data,
            base: self.base + off,
        })
    }
}

/// The ten offset slots of a JstfPriority table (§6.3.5.2), addressing
/// the shrink/extend × enable/disable × GSUB/GPOS mod-lists plus the two
/// JstfMax tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JstfMod {
    ShrinkageEnableGsub = 0,
    ShrinkageDisableGsub = 1,
    ShrinkageEnableGpos = 2,
    ShrinkageDisableGpos = 3,
    ShrinkageJstfMax = 4,
    ExtensionEnableGsub = 5,
    ExtensionDisableGsub = 6,
    ExtensionEnableGpos = 7,
    ExtensionDisableGpos = 8,
    ExtensionJstfMax = 9,
}

/// One justification-priority level (§6.3.5.2 JstfPriority table): ten
/// `Offset16` slots, any of which may be NULL.
#[derive(Debug, Clone, Copy)]
pub struct JstfPriority<'a> {
    data: &'a [u8],
    base: usize,
}

impl<'a> JstfPriority<'a> {
    /// The GSUB / GPOS lookup-list indices in the `Jstf{GSUB,GPOS}ModList`
    /// addressed by `slot`, in increasing order. Empty when the slot is
    /// NULL or names a `JstfMax` slot (which is a lookup *table* list, not
    /// a mod-list — use [`Self::jstf_max_lookup_count`] for those).
    pub fn mod_list(&self, slot: JstfMod) -> Vec<u16> {
        let mut out = Vec::new();
        // The two JstfMax slots are not ModLists.
        if matches!(slot, JstfMod::ShrinkageJstfMax | JstfMod::ExtensionJstfMax) {
            return out;
        }
        let Ok(off) = read_u16(self.data, self.base + slot as usize * 2) else {
            return out;
        };
        if off == 0 {
            return out;
        }
        let at = self.base + off as usize;
        let Ok(count) = read_u16(self.data, at) else {
            return out;
        };
        for i in 0..count as usize {
            match read_u16(self.data, at + 2 + i * 2) {
                Ok(idx) => out.push(idx),
                Err(_) => break,
            }
        }
        out
    }

    /// Number of lookup tables in the `JstfMax` slot (§6.3.5.2 JstfMax),
    /// or `None` when that slot is NULL / not a JstfMax slot. The JstfMax
    /// lookups themselves are GPOS-format lookup tables stored inline in
    /// the JSTF table; only their count is surfaced here.
    pub fn jstf_max_lookup_count(&self, slot: JstfMod) -> Option<usize> {
        if !matches!(slot, JstfMod::ShrinkageJstfMax | JstfMod::ExtensionJstfMax) {
            return None;
        }
        let off = read_u16(self.data, self.base + slot as usize * 2).ok()? as usize;
        if off == 0 {
            return None;
        }
        Some(read_u16(self.data, self.base + off).ok()? as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(v: u16) -> [u8; 2] {
        v.to_be_bytes()
    }

    /// Build a JSTF table with one script ("DFLT") carrying a default
    /// JstfLangSys with one priority level. That priority enables one
    /// GSUB lookup (index 7) for shrinkage and lists two extender glyphs.
    fn build_jstf() -> Vec<u8> {
        let mut d = Vec::new();
        // Header.
        d.extend_from_slice(&be(1)); // major
        d.extend_from_slice(&be(0)); // minor
        d.extend_from_slice(&be(1)); // jstfScriptCount
        d.extend_from_slice(b"DFLT"); // tag
        let script_off_pos = d.len();
        d.extend_from_slice(&be(0)); // scriptOffset (patched)

        // JstfScript.
        let script_base = d.len();
        let ext_off_pos = d.len();
        d.extend_from_slice(&be(0)); // extenderGlyphOffset (patched)
        let deflang_off_pos = d.len();
        d.extend_from_slice(&be(0)); // defJstfLangSysOffset (patched)
        d.extend_from_slice(&be(0)); // jstfLangSysCount

        // ExtenderGlyph table.
        let ext_at = d.len();
        d.extend_from_slice(&be(2)); // glyphCount
        d.extend_from_slice(&be(33));
        d.extend_from_slice(&be(34));
        d[ext_off_pos..ext_off_pos + 2].copy_from_slice(&be((ext_at - script_base) as u16));

        // Default JstfLangSys.
        let lang_at = d.len();
        d.extend_from_slice(&be(1)); // priorityCount
        let prio_off_pos = d.len();
        d.extend_from_slice(&be(0)); // priorityOffsets[0] (patched)
        d[deflang_off_pos..deflang_off_pos + 2]
            .copy_from_slice(&be((lang_at - script_base) as u16));

        // JstfPriority (10 offset slots).
        let prio_at = d.len();
        // slot 0 (shrinkageEnableGSUB) -> a ModList; rest NULL.
        let modlist_off_pos = d.len();
        d.extend_from_slice(&be(0)); // slot 0 (patched)
        for _ in 1..10 {
            d.extend_from_slice(&be(0));
        }
        d[prio_off_pos..prio_off_pos + 2].copy_from_slice(&be((prio_at - lang_at) as u16));

        // JstfGSUBModList for slot 0.
        let modlist_at = d.len();
        d.extend_from_slice(&be(1)); // lookupCount
        d.extend_from_slice(&be(7)); // gsubLookupIndices[0]
        d[modlist_off_pos..modlist_off_pos + 2].copy_from_slice(&be((modlist_at - prio_at) as u16));

        // Patch script offset.
        d[script_off_pos..script_off_pos + 2].copy_from_slice(&be(script_base as u16));
        d
    }

    #[test]
    fn jstf_navigation() {
        let data = build_jstf();
        let t = JstfTable::parse(&data).expect("parse");
        assert_eq!(t.script_count(), 1);
        assert_eq!(t.script_tags().collect::<Vec<_>>(), vec![*b"DFLT"]);

        let script = t.script(b"DFLT").expect("script");
        assert_eq!(script.extender_glyphs(), vec![33, 34]);
        assert_eq!(script.lang_sys_count(), 0);

        let lang = script.default_lang_sys().expect("default langsys");
        assert_eq!(lang.priority_count(), 1);

        let prio = lang.priority(0).expect("priority 0");
        assert_eq!(prio.mod_list(JstfMod::ShrinkageEnableGsub), vec![7]);
        // Other slots are NULL.
        assert!(prio.mod_list(JstfMod::ExtensionEnableGpos).is_empty());
        assert!(prio
            .jstf_max_lookup_count(JstfMod::ShrinkageJstfMax)
            .is_none());
        // Out-of-range priority.
        assert!(lang.priority(1).is_none());
    }

    #[test]
    fn rejects_wrong_version() {
        let mut data = vec![0u8; 6];
        data[1] = 3; // major = 3
        assert!(JstfTable::parse(&data).is_err());
    }

    #[test]
    fn unknown_script_is_none() {
        let data = build_jstf();
        let t = JstfTable::parse(&data).expect("parse");
        assert!(t.script(b"arab").is_none());
    }
}
