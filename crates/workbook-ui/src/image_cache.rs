//! Bounded LRU cache for decoded workbook images.
//!
//! The old cache was a plain map keyed by `workbook:asset` and was cleared on
//! every question switch. That forced GPUI to decode the same asset again for
//! every question. This cache keeps images across questions and across
//! workbooks, bounds the number of entries and the estimated decoded memory,
//! and treats a changed asset fingerprint as a new image.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui_kit::Image;

/// Estimated decoded memory budget. Assets are limited to 24 million pixels
/// (96 MB at RGBA8) and 50 assets per workbook, so a cache without a budget
/// could hold several gigabytes. 64 MiB keeps roughly a screenful of large
/// diagrams or dozens of smaller ones without unbounded growth.
pub(crate) const DEFAULT_MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
/// Hard entry bound even for tiny images (e.g. many small icons).
pub(crate) const DEFAULT_MAX_ENTRIES: usize = 24;

/// Identity of a cached image. `fingerprint` covers the asset bytes and
/// metadata, so replacing an asset without changing its ID cannot return the
/// old pixels.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ImageKey {
    pub workbook_id: String,
    pub asset_id: String,
    pub fingerprint: u64,
}

impl ImageKey {
    pub(crate) fn new(workbook_id: &str, asset_id: &str, fingerprint: u64) -> Self {
        Self {
            workbook_id: workbook_id.to_string(),
            asset_id: asset_id.to_string(),
            fingerprint,
        }
    }
}

/// Estimated decoded RGBA8 memory for an asset. The domain rejects images
/// above [`workbook_domain::MAX_IMAGE_PIXELS`], so this cannot overflow on
/// valid input; the saturating math keeps the cache safe for malformed data.
pub(crate) fn decoded_bytes(width: u32, height: u32) -> usize {
    let pixels = u64::from(width.max(1)) * u64::from(height.max(1));
    (pixels.saturating_mul(4)).min(usize::MAX as u64) as usize
}

struct Entry {
    image: Arc<Image>,
    bytes: usize,
    tick: u64,
}

pub(crate) struct ImageCache {
    entries: BTreeMap<ImageKey, Entry>,
    tick: u64,
    bytes: usize,
    max_bytes: usize,
    max_entries: usize,
    /// Images evicted from the map but still owned by GPUI's own asset cache.
    /// The app drains this list with an `App` handle and calls
    /// `Image::remove_asset`, which is what actually frees the decoded
    /// texture. Rendering cannot do that because it has no `App`.
    pending_removals: Vec<Arc<Image>>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_DECODED_BYTES, DEFAULT_MAX_ENTRIES)
    }
}

impl ImageCache {
    pub(crate) fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            tick: 0,
            bytes: 0,
            max_bytes: max_bytes.max(1),
            max_entries: max_entries.max(1),
            pending_removals: Vec::new(),
        }
    }

    /// Looks up an entry and marks it as most recently used.
    pub(crate) fn get(&mut self, key: &ImageKey) -> Option<Arc<Image>> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.entries.get_mut(key)?;
        entry.tick = tick;
        Some(entry.image.clone())
    }

    /// Returns a cached image or creates one. If an asset with the same
    /// workbook/asset ID but a different fingerprint exists, it is removed
    /// before the new entry is inserted.
    pub(crate) fn get_or_insert_with(
        &mut self,
        key: ImageKey,
        bytes: usize,
        create: impl FnOnce() -> Arc<Image>,
    ) -> Arc<Image> {
        if let Some(image) = self.get(&key) {
            return image;
        }

        self.remove_stale_versions(&key);

        let image = create();
        self.tick += 1;
        let previous = self.entries.insert(
            key,
            Entry {
                image: image.clone(),
                bytes,
                tick: self.tick,
            },
        );
        if let Some(previous) = previous {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
            self.pending_removals.push(previous.image);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.trim();
        image
    }

    /// Drops every entry for one workbook. Used on delete and re-import so a
    /// stale image can never be shown for that workbook.
    pub(crate) fn invalidate_workbook(&mut self, workbook_id: &str) {
        let keys: Vec<ImageKey> = self
            .entries
            .keys()
            .filter(|key| key.workbook_id == workbook_id)
            .cloned()
            .collect();
        for key in keys {
            self.remove(&key);
        }
    }

    /// Drops every entry. Used on import/restore when the underlying files may
    /// all have changed.
    pub(crate) fn clear(&mut self) {
        for (_, entry) in std::mem::take(&mut self.entries) {
            self.pending_removals.push(entry.image);
        }
        self.bytes = 0;
    }

    /// Enforces the byte and entry limits. One oversized image may stay: it is
    /// about to be drawn, and evicting it immediately would decode it again on
    /// the next frame.
    pub(crate) fn trim(&mut self) {
        while (self.bytes > self.max_bytes || self.entries.len() > self.max_entries)
            && self.entries.len() > 1
        {
            let Some(key) = self.least_recently_used() else {
                break;
            };
            self.remove(&key);
        }
    }

    /// Returns and clears the images evicted since the last call.
    pub(crate) fn take_removals(&mut self) -> Vec<Arc<Image>> {
        std::mem::take(&mut self.pending_removals)
    }

    fn least_recently_used(&self) -> Option<ImageKey> {
        self.entries
            .iter()
            .min_by_key(|(_, entry)| entry.tick)
            .map(|(key, _)| key.clone())
    }

    fn remove_stale_versions(&mut self, key: &ImageKey) {
        let stale: Vec<ImageKey> = self
            .entries
            .keys()
            .filter(|existing| {
                existing.workbook_id == key.workbook_id
                    && existing.asset_id == key.asset_id
                    && existing.fingerprint != key.fingerprint
            })
            .cloned()
            .collect();
        for stale in stale {
            self.remove(&stale);
        }
    }

    fn remove(&mut self, key: &ImageKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes = self.bytes.saturating_sub(entry.bytes);
            self.pending_removals.push(entry.image);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Image;

    fn key(workbook: &str, asset: &str, fingerprint: u64) -> ImageKey {
        ImageKey::new(workbook, asset, fingerprint)
    }

    fn image() -> Arc<Image> {
        Arc::new(Image::empty())
    }

    #[test]
    fn reuses_an_image_across_question_switches() {
        let mut cache = ImageCache::default();
        let key = key("w1", "diagram", 7);
        let first = cache.get_or_insert_with(key.clone(), 100, image);
        let second = cache.get_or_insert_with(key, 100, || {
            panic!("an existing entry must not be created again")
        });
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 100);
        assert!(cache.take_removals().is_empty());
    }

    #[test]
    fn evicts_the_least_recently_used_entry() {
        let mut cache = ImageCache::new(10_000, 2);
        let a = key("w1", "a", 1);
        let b = key("w1", "b", 1);
        let c = key("w1", "c", 1);
        cache.get_or_insert_with(a.clone(), 10, image);
        cache.get_or_insert_with(b.clone(), 10, image);
        cache.get(&a);
        cache.get_or_insert_with(c.clone(), 10, image);
        assert!(cache.get(&a).is_some());
        assert!(cache.get(&b).is_none());
        assert!(cache.get(&c).is_some());
        assert_eq!(cache.take_removals().len(), 1);
    }

    #[test]
    fn a_changed_asset_content_never_returns_the_old_image() {
        let mut cache = ImageCache::default();
        let old = key("w1", "diagram", 1);
        let new = key("w1", "diagram", 2);
        let first = cache.get_or_insert_with(old, 10, image);
        let second = cache.get_or_insert_with(new.clone(), 10, image);
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&new).is_some());
        // The old texture is queued for removal from GPUI's asset cache.
        assert_eq!(cache.take_removals().len(), 1);
    }

    #[test]
    fn the_same_asset_id_in_another_workbook_is_a_separate_entry() {
        let mut cache = ImageCache::default();
        let first = cache.get_or_insert_with(key("w1", "diagram", 9), 10, image);
        let second = cache.get_or_insert_with(key("w2", "diagram", 9), 10, image);
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key("w1", "diagram", 9)).is_some());
        assert!(cache.get(&key("w2", "diagram", 9)).is_some());
    }

    #[test]
    fn invalidating_one_workbook_keeps_other_workbooks() {
        let mut cache = ImageCache::default();
        let a = key("w1", "a", 1);
        let b = key("w2", "b", 1);
        cache.get_or_insert_with(a.clone(), 10, image);
        cache.get_or_insert_with(b.clone(), 10, image);
        cache.invalidate_workbook("w1");
        assert!(cache.get(&a).is_none());
        assert!(cache.get(&b).is_some());
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.take_removals().len(), 1);
    }

    #[test]
    fn clear_drops_every_entry_and_queues_texture_removal() {
        let mut cache = ImageCache::default();
        cache.get_or_insert_with(key("w1", "a", 1), 10, image);
        cache.get_or_insert_with(key("w2", "b", 1), 10, image);
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.bytes(), 0);
        assert_eq!(cache.take_removals().len(), 2);
    }

    #[test]
    fn the_byte_budget_bounds_growth() {
        let mut cache = ImageCache::new(100, 24);
        for index in 0..10 {
            cache.get_or_insert_with(key("w1", &format!("asset-{index}"), 1), 60, image);
        }
        // 60 bytes each, 100-byte budget: only the newest entry can stay.
        assert_eq!(cache.len(), 1);
        assert!(cache.bytes() <= 100);
        assert_eq!(cache.take_removals().len(), 9);
    }

    #[test]
    fn a_single_oversized_image_is_kept_until_replaced() {
        let mut cache = ImageCache::new(50, 24);
        let oversized = key("w1", "huge", 1);
        cache.get_or_insert_with(oversized.clone(), 500, image);
        assert!(cache.get(&oversized).is_some());
        assert_eq!(cache.len(), 1);
        cache.get_or_insert_with(key("w1", "small", 1), 10, image);
        assert!(
            cache.get(&oversized).is_none(),
            "oversized entry must be evicted"
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_failed_decode_is_not_remembered() {
        // Decoding happens in GPUI (`Image::from_bytes` plus the renderer's
        // fallback). The cache must not grow when the caller never inserts.
        let mut cache = ImageCache::default();
        assert!(cache.get(&key("w1", "missing", 1)).is_none());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn decoded_memory_accounts_for_rgba_pixels() {
        assert_eq!(decoded_bytes(1, 1), 4);
        assert_eq!(decoded_bytes(640, 480), 640 * 480 * 4);
        // 24 million pixels is the largest accepted import (96 MB decoded).
        assert_eq!(decoded_bytes(6000, 4000), 96_000_000);
        assert_eq!(decoded_bytes(0, 0), 4);
    }
}
