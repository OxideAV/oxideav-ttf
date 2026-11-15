//! A small lock-free memo table for pure `u32 -> Option<u16>` font
//! queries (character → glyph id, glyph → advance, glyph pair → kerning
//! adjustment), shared by every thread using a [`Font`](crate::Font).
//!
//! Direct-mapped: each key hashes to one slot, a newer key overwrites an
//! older one that lands on the same slot. A slot is a single `AtomicU64`
//! holding the key together with its value, so a reader either sees a
//! complete entry for *some* key — which it compares against its own —
//! or an empty slot. There is nothing to lock and nothing to tear; a
//! lost race costs one recomputation. Because the memoised function is
//! pure, a hit is indistinguishable from recomputing.
//!
//! The table is not allocated until the memo has been asked
//! [`WARM_UP`] times: a font opened for one short job never pays for
//! it, a font that is kept and reused crosses the threshold at once.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Queries answered by computing directly before the table is
/// allocated.
const WARM_UP: u32 = 256;

pub(crate) struct Memo {
    slots: OnceLock<Box<[AtomicU64]>>,
    /// log2 of the slot count.
    bits: u32,
    /// Queries seen while the table is unallocated.
    warm_up: AtomicU32,
}

impl Memo {
    /// A memo with `2^bits` slots (8 bytes each), allocated once it has
    /// been queried [`WARM_UP`] times. `bits` must be in `1..=24`.
    pub(crate) const fn with_bits(bits: u32) -> Self {
        Self {
            slots: OnceLock::new(),
            bits,
            warm_up: AtomicU32::new(0),
        }
    }

    /// The memoised value of `compute` for `key`, computing and storing
    /// it on a miss. `compute` must be a pure function of `key`.
    #[inline]
    pub(crate) fn get_or_compute(
        &self,
        key: u32,
        compute: impl FnOnce() -> Option<u16>,
    ) -> Option<u16> {
        let slots = match self.slots.get() {
            Some(slots) => slots,
            None => {
                if self.warm_up.fetch_add(1, Ordering::Relaxed) < WARM_UP {
                    return compute();
                }
                self.slots.get_or_init(|| {
                    (0..1usize << self.bits)
                        .map(|_| AtomicU64::new(0))
                        .collect()
                })
            }
        };
        // Fibonacci hashing: the top bits of key * 2^32 / phi.
        let index = (key.wrapping_mul(0x9E37_79B9) >> (32 - self.bits)) as usize;
        let slot = &slots[index];
        // Entry layout: (key + 1) in bits 17.., "value present" in bit
        // 16, the value in bits 0..16. Zero is the empty slot (the
        // tag `key + 1` is never zero).
        let tag = (key as u64 + 1) << 17;
        let entry = slot.load(Ordering::Relaxed);
        if entry & !0x1_FFFF == tag {
            return (entry & 0x1_0000 != 0).then_some(entry as u16);
        }
        let value = compute();
        let packed = match value {
            Some(v) => tag | 0x1_0000 | v as u64,
            None => tag,
        };
        slot.store(packed, Ordering::Relaxed);
        value
    }
}

impl core::fmt::Debug for Memo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(if self.slots.get().is_some() {
            "Memo(allocated)"
        } else {
            "Memo(empty)"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_stays_unallocated_through_the_warm_up() {
        let memo = Memo::with_bits(4);
        for key in 0..WARM_UP {
            assert_eq!(
                memo.get_or_compute(key % 3, || Some(key as u16)),
                Some(key as u16)
            );
            assert!(memo.slots.get().is_none());
        }
        assert_eq!(memo.get_or_compute(1, || Some(9)), Some(9));
        assert!(memo.slots.get().is_some());
        assert_eq!(memo.get_or_compute(1, || unreachable!()), Some(9));
    }

    #[test]
    fn memo_returns_what_the_function_returns() {
        let memo = Memo::with_bits(10);
        let f = |k: u32| (!k.is_multiple_of(7)).then_some((k.wrapping_mul(31) % 65_521) as u16);
        // Far more keys than slots, each asked several times, so hits,
        // misses and evictions all occur; include the extreme keys.
        let keys = (0..5_000u32)
            .map(|i| i.wrapping_mul(2_654_435_761) % 0x11_0000)
            .chain([0, 1, 0x10_FFFF, u32::MAX]);
        for key in keys {
            for _ in 0..3 {
                assert_eq!(memo.get_or_compute(key, || f(key)), f(key), "key {key}");
            }
        }
        // A hit does not call the function again (the table is live:
        // the loop above ran far past the warm-up).
        memo.get_or_compute(42, || Some(7));
        assert_eq!(memo.get_or_compute(42, || unreachable!()), Some(7));
        assert_eq!(memo.get_or_compute(49, || None), None);
        assert_eq!(memo.get_or_compute(49, || unreachable!()), None);
    }
}
