//! A single cancellable search worker. New queries replace queued work for a
//! document; at most one completed result waits for the UI to collect it.
use super::Pattern;
use eframe::egui;
use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Key {
    pub document: u64,
    pub revision: u64,
    pub byte_len: usize,
    pub pattern: Pattern,
}

pub(super) struct ResultEntry {
    pub key: Key,
    pub matches: Arc<[Range<usize>]>,
    pub error: Option<String>,
}

struct Job {
    key: Key,
    text: Arc<str>,
    cancel: Arc<AtomicBool>,
    ready_at: Instant,
}

#[derive(Default)]
struct State {
    latest: HashMap<u64, (Key, Arc<AtomicBool>)>,
    queued: VecDeque<Job>,
    complete: Option<ResultEntry>,
    stopped: bool,
}

pub(super) struct Worker {
    shared: Arc<(Mutex<State>, Condvar)>,
}

impl Worker {
    pub fn new(ctx: egui::Context) -> Self {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker_shared = shared.clone();
        std::thread::Builder::new()
            .name("morrow-find".into())
            .spawn(move || {
                loop {
                    let (mutex, ready) = &*worker_shared;
                    let job = {
                        let mut state = mutex.lock().unwrap_or_else(|poison| poison.into_inner());
                        while !state.stopped {
                            if state.complete.is_none()
                                && let Some(job) = state.queued.front()
                            {
                                let remaining =
                                    job.ready_at.saturating_duration_since(Instant::now());
                                if remaining.is_zero() {
                                    break;
                                }
                                state = ready
                                    .wait_timeout(state, remaining)
                                    .unwrap_or_else(|poison| poison.into_inner())
                                    .0;
                            } else {
                                state = ready
                                    .wait(state)
                                    .unwrap_or_else(|poison| poison.into_inner());
                            }
                        }
                        if state.stopped {
                            break;
                        }
                        state.queued.pop_front().expect("queued search")
                    };
                    let result = crate::search::search_cancellable(
                        &job.text,
                        &job.key.pattern.query,
                        job.key.pattern.case_sensitive,
                        job.key.pattern.whole_word,
                        job.key.pattern.regex,
                        &job.cancel,
                    );
                    if job.cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    let (matches, error) = match result {
                        Ok(matches) => (Arc::from(matches), None),
                        Err(error) => (Arc::from([]), Some(error)),
                    };
                    if publish(&worker_shared, job, matches, error) {
                        ctx.request_repaint();
                    }
                }
            })
            .expect("start file-search worker");
        Self { shared }
    }

    pub fn request(&self, key: Key, text: &str) {
        let (mutex, ready) = &*self.shared;
        let mut state = mutex.lock().unwrap_or_else(|poison| poison.into_inner());
        if state
            .latest
            .get(&key.document)
            .is_some_and(|(previous, _)| *previous == key)
        {
            return;
        }
        if let Some((_, cancel)) = state.latest.remove(&key.document) {
            cancel.store(true, Ordering::Relaxed);
        }
        state.queued.retain(|job| job.key.document != key.document);
        if state
            .complete
            .as_ref()
            .is_some_and(|result| result.key.document == key.document)
        {
            state.complete = None;
        }
        // The UI has at most eight panes. Keep a bounded cushion for a prompt
        // and recently requested documents, rather than copying every edit.
        while state.queued.len() >= 16 {
            if let Some(job) = state.queued.pop_front() {
                job.cancel.store(true, Ordering::Relaxed);
                state.latest.remove(&job.key.document);
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        state
            .latest
            .insert(key.document, (key.clone(), cancel.clone()));
        state.queued.push_back(Job {
            key,
            text: Arc::from(text),
            cancel,
            ready_at: Instant::now() + Duration::from_millis(60),
        });
        ready.notify_one();
    }

    pub fn take_result(&self) -> Option<ResultEntry> {
        let (mutex, ready) = &*self.shared;
        let mut state = mutex.lock().unwrap_or_else(|poison| poison.into_inner());
        let result = state.complete.take()?;
        if state
            .latest
            .get(&result.key.document)
            .is_some_and(|(key, _)| *key == result.key)
        {
            state.latest.remove(&result.key.document);
        }
        ready.notify_one();
        Some(result)
    }

    pub fn cancel(&self, document: u64) {
        let (mutex, ready) = &*self.shared;
        let mut state = mutex.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some((_, cancel)) = state.latest.remove(&document) {
            cancel.store(true, Ordering::Relaxed);
        }
        state.queued.retain(|job| job.key.document != document);
        if state
            .complete
            .as_ref()
            .is_some_and(|result| result.key.document == document)
        {
            state.complete = None;
        }
        ready.notify_one();
    }

    pub fn pending(&self, document: u64) -> bool {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .latest
            .contains_key(&document)
    }

    #[cfg(test)]
    pub fn manual() -> Self {
        Self {
            shared: Arc::new((Mutex::new(State::default()), Condvar::new())),
        }
    }

    #[cfg(test)]
    pub fn finish(&self, document: u64, matches: Vec<Range<usize>>) {
        let job = {
            let mut state = self.shared.0.lock().unwrap();
            let index = state
                .queued
                .iter()
                .position(|job| job.key.document == document)
                .unwrap();
            state.queued.remove(index).unwrap()
        };
        assert!(publish(&self.shared, job, Arc::from(matches), None));
    }
}

fn publish(
    shared: &(Mutex<State>, Condvar),
    job: Job,
    matches: Arc<[Range<usize>]>,
    error: Option<String>,
) -> bool {
    let mut state = shared.0.lock().unwrap_or_else(|poison| poison.into_inner());
    if state.stopped
        || job.cancel.load(Ordering::Relaxed)
        || !state
            .latest
            .get(&job.key.document)
            .is_some_and(|(key, cancel)| *key == job.key && Arc::ptr_eq(cancel, &job.cancel))
    {
        return false;
    }
    state.complete = Some(ResultEntry {
        key: job.key,
        matches,
        error,
    });
    true
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (mutex, ready) = &*self.shared;
        let mut state = mutex.lock().unwrap_or_else(|poison| poison.into_inner());
        state.stopped = true;
        for (_, cancel) in state.latest.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        state.queued.clear();
        state.complete = None;
        ready.notify_one();
    }
}

#[cfg(test)]
// Fixtures contain match ranges, not integer sequences.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    fn key(query: &str) -> Key {
        Key {
            document: 1,
            revision: 0,
            byte_len: 4,
            pattern: Pattern::vim(query.into()),
        }
    }
    #[test]
    fn replaced_or_cancelled_running_job_cannot_publish() {
        let worker = Worker::manual();
        worker.request(key("a"), "aaaa");
        let first = worker.shared.0.lock().unwrap().queued.pop_front().unwrap();
        worker.request(key("b"), "aaaa");
        assert!(!publish(&worker.shared, first, Arc::from(vec![0..1]), None));
        let second = worker.shared.0.lock().unwrap().queued.pop_front().unwrap();
        worker.cancel(1);
        assert!(!publish(
            &worker.shared,
            second,
            Arc::from(vec![1..2]),
            None
        ));
        assert!(worker.take_result().is_none());
        worker.request(key("c"), "aaaa");
        worker.finish(1, vec![2..3]);
        assert_eq!(&*worker.take_result().unwrap().matches, &[2..3]);
    }
    #[test]
    fn cancelled_ready_result_is_not_delivered_and_duplicate_requests_reuse_work() {
        let worker = Worker::manual();
        worker.request(key("a"), "aaaa");
        worker.request(key("a"), "aaaa");
        assert_eq!(worker.shared.0.lock().unwrap().queued.len(), 1);
        worker.finish(1, vec![0..1]);
        worker.cancel(1);
        assert!(worker.take_result().is_none());
        assert!(!worker.pending(1));
    }
}
