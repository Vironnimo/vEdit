//! Byte-bounded LRU of complete search results. One oversized result stays
//! available for navigation; smaller split views must not evict it every frame.
use std::{collections::VecDeque, ops::Range, sync::Arc};

const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CACHE_DOCUMENTS: usize = 16;

pub(crate) type MatchResult = (Arc<[Range<usize>]>, Option<String>);

struct Entry<P> {
    document: u64,
    revision: u64,
    byte_len: usize,
    pattern: P,
    matches: Arc<[Range<usize>]>,
    error: Option<String>,
    bytes: usize,
}

pub(crate) struct MatchCache<P> {
    entries: VecDeque<Entry<P>>,
    byte_budget: usize,
}

impl<P> Default for MatchCache<P> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            byte_budget: MAX_CACHE_BYTES,
        }
    }
}

impl<P: PartialEq> MatchCache<P> {
    pub(crate) fn get(
        &mut self,
        document: u64,
        revision: u64,
        byte_len: usize,
        pattern: &P,
    ) -> Option<MatchResult> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.document == document)?;
        let entry = self.entries.remove(index)?;
        if entry.revision != revision || entry.byte_len != byte_len || &entry.pattern != pattern {
            // Release an old dense result before its replacement is allocated.
            return None;
        }
        let result = (entry.matches.clone(), entry.error.clone());
        self.entries.push_back(entry);
        Some(result)
    }

    pub(crate) fn insert(
        &mut self,
        document: u64,
        revision: u64,
        byte_len: usize,
        pattern: P,
        matches: Arc<[Range<usize>]>,
        error: Option<String>,
    ) {
        self.invalidate(document);
        let bytes = matches
            .len()
            .saturating_mul(std::mem::size_of::<Range<usize>>())
            .saturating_add(error.as_ref().map_or(0, String::len));
        if bytes > self.byte_budget {
            // Complete dense results can exceed the normal budget. Keep only
            // the latest such document, plus at most 64 MiB of smaller results.
            self.entries.retain(|entry| entry.bytes <= self.byte_budget);
        }
        self.entries.push_back(Entry {
            document,
            revision,
            byte_len,
            pattern,
            matches,
            error,
            bytes,
        });
        while self.entries.len() > MAX_CACHE_DOCUMENTS {
            self.entries.pop_front();
        }
        while self
            .entries
            .iter()
            .filter(|entry| entry.bytes <= self.byte_budget)
            .map(|entry| entry.bytes)
            .sum::<usize>()
            > self.byte_budget
        {
            let index = self
                .entries
                .iter()
                .position(|entry| entry.bytes <= self.byte_budget)
                .expect("normal budget exceeded");
            self.entries.remove(index);
        }
    }

    pub(crate) fn invalidate(&mut self, document: u64) {
        self.entries.retain(|entry| entry.document != document);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(bytes: usize) -> Arc<[Range<usize>]> {
        vec![0..1; bytes / std::mem::size_of::<Range<usize>>()].into()
    }

    #[test]
    fn reuse_checks_revision_length_and_pattern_without_losing_other_documents() {
        let mut cache = MatchCache::default();
        let first = ranges(16);
        cache.insert(1, 1, 1, "a", first.clone(), None);
        cache.insert(2, 1, 1, "a", ranges(16), None);
        assert!(Arc::ptr_eq(&cache.get(1, 1, 1, &"a").unwrap().0, &first));
        assert!(cache.get(1, 2, 1, &"a").is_none());
        assert!(cache.get(2, 1, 2, &"a").is_none());
        cache.insert(1, 2, 1, "a", first, None);
        assert!(cache.get(1, 2, 1, &"b").is_none());
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn byte_budget_evicts_least_recent_results_without_truncating_matches() {
        let mut cache = MatchCache {
            byte_budget: 64,
            ..MatchCache::default()
        };
        let chunk = ranges(32);
        cache.insert(1, 1, 1, "a", chunk.clone(), None);
        cache.insert(2, 1, 1, "a", chunk.clone(), None);
        cache.get(1, 1, 1, &"a").unwrap();
        cache.insert(3, 1, 1, "a", chunk.clone(), None);
        assert!(cache.get(2, 1, 1, &"a").is_none());
        assert!(Arc::ptr_eq(&cache.get(1, 1, 1, &"a").unwrap().0, &chunk));
        assert_eq!(
            cache.entries.iter().map(|entry| entry.bytes).sum::<usize>(),
            64
        );
    }

    #[test]
    fn oversized_document_survives_small_split_views_and_replaces_older_oversize() {
        let mut cache = MatchCache {
            byte_budget: 64,
            ..MatchCache::default()
        };
        let dense = ranges(80);
        cache.insert(1, 1, 1, "a", dense.clone(), None);
        cache.insert(2, 1, 1, "a", ranges(16), None);
        assert!(Arc::ptr_eq(&cache.get(1, 1, 1, &"a").unwrap().0, &dense));
        cache.insert(3, 1, 1, "a", dense.clone(), None);
        assert!(cache.get(1, 1, 1, &"a").is_none());
        assert!(cache.get(2, 1, 1, &"a").is_some());
        assert_eq!(cache.get(3, 1, 1, &"a").unwrap().0.len(), dense.len());
        assert!(
            cache.entries.iter().map(|entry| entry.bytes).sum::<usize>()
                <= 64 + dense.len() * std::mem::size_of::<Range<usize>>()
        );
    }
}
