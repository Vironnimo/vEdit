//! Cached worktree discovery for Source control and the project picker.
//!
//! Shows the active project's repository and its worktrees, including ones
//! that are not open as projects. Listings stay cached across project switches.
//! Open projects reuse their own status refreshes. Discovery starts one
//! `worktree list` per repository and one status
//! read per worktree that is not open, while either navigation surface is visible.
//! Request IDs keep a late worker from replacing a newer result.

use super::*;
use std::collections::HashSet;

/// Worktrees added or removed outside vEdit appear within this interval.
const LISTING_INTERVAL: Duration = Duration::from_secs(15);
/// Worktrees that are not open refresh at half the inactive project cadence.
const STATUS_INTERVAL: Duration = Duration::from_secs(30);
const MAX_JOBS: usize = 2;
const MAX_HEADLINES: usize = 512;

/// Compare worktree paths the way Windows and Git for Windows do: without the
/// verbatim prefix, with either separator and without case.
pub(in crate::app) fn path_key(path: &Path) -> String {
    let mut text = crate::paths::display(path);
    if cfg!(windows) {
        text = text.replace('/', "\\").to_lowercase();
    }
    while text.len() > 1 && (text.ends_with('\\') || text.ends_with('/')) && !text.ends_with(":\\")
    {
        text.pop();
    }
    text
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Target {
    /// Keyed by the seed path the listing runs in.
    Listing(String),
    /// Keyed by the worktree path.
    Status(String),
    Details(String),
}

struct Job {
    target: Target,
    started: Instant,
    discovery: bool,
}

struct Repository {
    /// Path key of the main worktree; one group per repository.
    key: String,
    seed: PathBuf,
    seed_key: String,
    entries: Vec<git::WorktreeEntry>,
    keys: Vec<String>,
    refreshed: Instant,
    request: u64,
    error: Option<String>,
}

impl Repository {
    fn covers(&self, key: &str) -> bool {
        self.seed_key == key || self.keys.iter().any(|candidate| candidate == key)
    }
}

struct StatusEntry {
    result: Result<git::WorktreeStatus, String>,
    refreshed: Instant,
    request: u64,
}

struct DetailsEntry {
    result: Result<ChangeDetails, String>,
    refreshed: Instant,
    request: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct ChangeDetails {
    files: usize,
    added: u64,
    removed: u64,
    binary: usize,
    unknown: usize,
}

fn read_change_details(path: &Path) -> Result<ChangeDetails, String> {
    let (status, _, _) = git::status_with_head(path).map_err(|error| error.to_string())?;
    let stats = git::working_tree_stats(path, &status.files).map_err(|error| error.to_string())?;
    Ok(change_details(&status.files, &stats))
}

fn change_details(files: &[git::FileStatus], stats: &git::WorkingTreeStats) -> ChangeDetails {
    let mut details = ChangeDetails {
        files: files.len(),
        ..Default::default()
    };
    for file in files {
        let mut binary = false;
        let mut unknown = file.is_conflicted();
        for (changed, comparison) in [
            (file.is_staged(), &stats.staged),
            (file.is_unstaged() || file.is_untracked(), &stats.unstaged),
        ] {
            if changed {
                if let Some(lines) = comparison.get(&file.path) {
                    details.added = details.added.saturating_add(lines.added);
                    details.removed = details.removed.saturating_add(lines.removed);
                    binary |= lines.binary;
                } else {
                    unknown = true;
                }
            }
        }
        details.binary += usize::from(binary);
        details.unknown += usize::from(unknown);
    }
    details
}

#[derive(Default)]
pub(in crate::app) struct RepositoryOverview {
    repositories: Vec<Repository>,
    /// Worktrees that are not open as projects, by path key.
    statuses: HashMap<String, StatusEntry>,
    /// Line counts are read only while a row is hovered, within the same job budget.
    details: HashMap<String, DetailsEntry>,
    headlines: HashMap<String, git::CommitHeadline>,
    headline_order: VecDeque<String>,
    /// Heads already asked for once, so a missing object cannot loop.
    requested_heads: HashSet<String>,
    failed: HashMap<String, (Instant, String)>,
    jobs: HashMap<u64, Job>,
    next_request: u64,
    refresh_requested: Option<Instant>,
}

/// One unit of background work. It owns everything it needs, so the UI
/// thread only plans and the worker runs Git.
pub(in crate::app) enum Work {
    Listing {
        request: u64,
        seed: PathBuf,
        /// Heads whose headlines are cached and need no process.
        cached: HashSet<String>,
        /// Open projects' heads that still lack a headline.
        extra: Vec<String>,
    },
    Status {
        request: u64,
        path: PathBuf,
        cached_head: Option<String>,
    },
    Details {
        request: u64,
        path: PathBuf,
    },
}

pub(in crate::app) enum OverviewResult {
    Listing {
        request: u64,
        result: Result<Listing, String>,
    },
    Status {
        request: u64,
        result: Result<StatusRead, String>,
    },
    Details {
        request: u64,
        result: Result<ChangeDetails, String>,
    },
}

pub(in crate::app) struct Listing {
    entries: Vec<git::WorktreeEntry>,
    headlines: Vec<git::CommitHeadline>,
}

pub(in crate::app) struct StatusRead {
    status: git::WorktreeStatus,
    headline: Option<git::CommitHeadline>,
}

impl Work {
    pub(in crate::app) fn run(self) -> OverviewResult {
        match self {
            Self::Listing {
                request,
                seed,
                cached,
                extra,
            } => OverviewResult::Listing {
                request,
                result: read_listing(&seed, &cached, extra),
            },
            Self::Status {
                request,
                path,
                cached_head,
            } => OverviewResult::Status {
                request,
                result: read_status(&path, cached_head.as_deref()),
            },
            Self::Details { request, path } => OverviewResult::Details {
                request,
                result: read_change_details(&path),
            },
        }
    }
}

/// One `worktree list` plus, only when a head is new, one `log` process.
fn read_listing(
    seed: &Path,
    cached: &HashSet<String>,
    extra: Vec<String>,
) -> Result<Listing, String> {
    let mut entries = git::worktree_list(seed).map_err(|error| error.to_string())?;
    for entry in &mut entries {
        // Project roots are canonical; resolve junctions and short names the
        // same way so an open worktree is recognized as that project.
        if entry.prunable.is_none()
            && let Ok(path) = fs::canonicalize(&entry.path)
        {
            entry.path = path;
        }
    }
    let mut wanted: Vec<String> = entries
        .iter()
        .filter_map(|entry| entry.head.clone())
        .chain(extra)
        .filter(|head| !cached.contains(head))
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    // A failed headline read leaves subjects blank rather than hiding worktrees.
    let headlines = git::commit_headlines(seed, &wanted).unwrap_or_default();
    Ok(Listing { entries, headlines })
}

/// One status process plus one `log` process when HEAD moved.
fn read_status(path: &Path, cached_head: Option<&str>) -> Result<StatusRead, String> {
    let status = git::worktree_status(path).map_err(|error| error.to_string())?;
    let headline = match &status.head {
        Some(head) if cached_head != Some(head.as_str()) => {
            git::commit_headlines(path, std::slice::from_ref(head))
                .ok()
                .and_then(|mut headlines| headlines.pop())
        }
        _ => None,
    };
    Ok(StatusRead { status, headline })
}

/// What the overview knows about an open project, taken from its own refreshes.
struct OpenProject<'a> {
    index: usize,
    current: bool,
    root_key: String,
    repository_key: Option<String>,
    repository: Option<&'a Path>,
    status: Option<&'a git::WorktreeStatus>,
    error: Option<&'a str>,
}

impl OpenProject<'_> {
    fn status(&self) -> Option<Result<git::WorktreeStatus, String>> {
        self.status
            .map(|status| Ok(status.clone()))
            .or_else(|| self.error.map(|error| Err(error.to_owned())))
    }

    fn maps_to(&self, key: &str) -> bool {
        self.root_key == key || self.repository_key.as_deref() == Some(key)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RowKind {
    Worktree,
    Bare,
    Missing(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Row {
    key: String,
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) name: String,
    linked: bool,
    pub(in crate::app) project: Option<usize>,
    current: bool,
    kind: RowKind,
    branch: Option<String>,
    head: Option<String>,
    locked: Option<String>,
    status: Option<Result<git::WorktreeStatus, String>>,
    details: Option<Result<ChangeDetails, String>>,
    headline: Option<git::CommitHeadline>,
    listing_error: Option<String>,
}

impl Row {
    fn copy_details(&self, now: i64) -> String {
        let mut lines = vec![
            self.name.clone(),
            crate::paths::display(&self.path),
            self.summary(),
        ];
        if let Some(head) = &self.head {
            lines.push(head.clone());
        }
        if let Some(Ok(status)) = &self.status
            && let Some(upstream) = &status.tracking.upstream
        {
            lines.push(format!("Tracking {upstream}"));
        }
        match &self.details {
            Some(Ok(details)) => lines.push(format!("{} files changed · +{} −{}\n{} binary files · {} files with unavailable line counts", details.files, details.added, details.removed, details.binary, details.unknown)),
            Some(Err(error)) => lines.push(format!("Line counts unavailable: {error}")),
            None if self.openable() => lines.push("Loading line counts…".into()),
            None => {}
        }
        if let Some(Err(error)) = &self.status {
            lines.push(format!("Status unavailable: {error}"));
        }
        if let Some(headline) = &self.headline {
            lines.push(format!(
                "Last commit: {}\n{} · {}\n{}",
                headline.subject,
                headline.author,
                relative_time(headline.timestamp, now),
                headline.hash
            ));
        }
        match &self.kind {
            RowKind::Bare => lines.push("Bare repository without a working tree".into()),
            RowKind::Missing(reason) => {
                lines.push(format!("Worktree directory is missing: {reason}"))
            }
            RowKind::Worktree => {}
        }
        if let Some(reason) = &self.locked {
            lines.push(format!("Locked: {reason}"));
        }
        if let Some(error) = &self.listing_error {
            lines.push(format!("Worktree list unavailable: {error}"));
        }
        lines.join("\n")
    }

    pub(in crate::app) fn openable(&self) -> bool {
        self.kind == RowKind::Worktree
    }

    pub(in crate::app) fn label(&self) -> String {
        format!("Worktree {}", self.name)
    }

    pub(in crate::app) fn badge(&self) -> &str {
        match self.kind {
            RowKind::Worktree => "worktree",
            RowKind::Bare => "bare",
            RowKind::Missing(_) => "missing",
        }
    }

    pub(in crate::app) fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(branch) = &self.branch {
            parts.push(branch.clone());
        } else if let Some(head) = &self.head {
            parts.push(short_hash(head).into());
        }
        if let Some(Ok(status)) = &self.status {
            if status.changes > 0 {
                parts.push(format!("{} changed", status.changes));
            }
            if status.conflicts > 0 {
                parts.push(format!("{} conflicts", status.conflicts));
            }
            if let Some((sync, _)) = sync_marker(status) {
                parts.push(sync);
            }
        }
        parts.join(" · ")
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::paths::display(path))
}

impl RepositoryOverview {
    fn headline(&self, head: Option<&String>) -> Option<git::CommitHeadline> {
        head.and_then(|head| self.headlines.get(head)).cloned()
    }

    fn remember_headline(&mut self, headline: git::CommitHeadline) {
        if self.headlines.contains_key(&headline.hash) {
            return;
        }
        while self.headlines.len() >= MAX_HEADLINES {
            match self.headline_order.pop_front() {
                Some(oldest) => {
                    self.headlines.remove(&oldest);
                }
                None => self.headlines.clear(),
            }
        }
        self.headline_order.push_back(headline.hash.clone());
        self.headlines.insert(headline.hash.clone(), headline);
    }

    /// Commits, fetches and branch switches change other worktrees' upstream
    /// divergence and listings too, so everything becomes due once.
    pub(in crate::app) fn request_refresh(&mut self) {
        self.refresh_requested = Some(Instant::now());
    }

    fn requested_after(&self, refreshed: Instant) -> bool {
        self.refresh_requested.is_some_and(|at| at > refreshed)
    }

    fn start(&mut self, target: Target, discovery: bool, now: Instant) -> u64 {
        self.next_request += 1;
        let request = self.next_request;
        self.jobs.insert(
            request,
            Job {
                target,
                started: now,
                discovery,
            },
        );
        request
    }

    /// Choose the next background reads. Open worktrees never get a status
    /// read here; their projects' own refreshes already provide one.
    fn plan(
        &mut self,
        seeds: &[(String, PathBuf)],
        open: &HashMap<String, Option<String>>,
        now: Instant,
    ) -> Vec<Work> {
        let seed_keys: HashSet<&str> = seeds.iter().map(|(key, _)| key.as_str()).collect();
        self.repositories.retain(|repository| {
            seed_keys.contains(repository.seed_key.as_str())
                || repository
                    .keys
                    .iter()
                    .any(|key| seed_keys.contains(key.as_str()))
        });
        let listed: HashSet<&String> = self
            .repositories
            .iter()
            .flat_map(|repository| &repository.keys)
            .collect();
        // An open worktree keeps its last overview status until its project's
        // own refresh replaces it; it gets no further reads from here.
        self.statuses.retain(|key, _| listed.contains(key));
        self.details.retain(|key, _| listed.contains(key));
        self.failed
            .retain(|key, _| seed_keys.contains(key.as_str()));
        if self.requested_heads.len() > MAX_HEADLINES {
            self.requested_heads.clear();
        }
        let mut budget = MAX_JOBS.saturating_sub(self.jobs.len());
        let mut work = Vec::new();
        if budget == 0 {
            return work;
        }
        let in_flight: HashSet<Target> = self.jobs.values().map(|job| job.target.clone()).collect();
        // At most one listing for a repository that is not known yet: two
        // seeds in one repository must not both list it.
        let discovery = seeds
            .iter()
            .filter(|_| !self.jobs.values().any(|job| job.discovery))
            .find(|(key, _)| self.needs_discovery(key, &in_flight, now));
        if let Some((key, seed)) = discovery {
            let extra = self.missing_heads(open, |candidate| candidate == key.as_str());
            self.requested_heads.extend(extra.iter().cloned());
            let request = self.start(Target::Listing(key.clone()), true, now);
            work.push(Work::Listing {
                request,
                seed: seed.clone(),
                cached: HashSet::new(),
                extra,
            });
            budget -= 1;
        }
        let mut due: Vec<(Instant, usize)> = self
            .repositories
            .iter()
            .enumerate()
            .filter(|(_, repository)| {
                !in_flight.contains(&Target::Listing(repository.seed_key.clone()))
                    && (now.saturating_duration_since(repository.refreshed) >= LISTING_INTERVAL
                        || self.requested_after(repository.refreshed)
                        || !self
                            .missing_heads(open, |key| repository.covers(key))
                            .is_empty())
            })
            .map(|(index, repository)| (repository.refreshed, index))
            .collect();
        due.sort_unstable();
        for (_, index) in due.into_iter().take(budget) {
            let repository = &self.repositories[index];
            // The original seed may have closed; any open worktree can list.
            let Some((seed_key, seed)) = seeds
                .iter()
                .find(|(key, _)| *key == repository.seed_key)
                .or_else(|| seeds.iter().find(|(key, _)| repository.covers(key)))
                .cloned()
            else {
                continue;
            };
            let mut cached = HashSet::new();
            for (entry, key) in repository.entries.iter().zip(&repository.keys) {
                let status_head = self
                    .statuses
                    .get(key)
                    .and_then(|status| status.result.as_ref().ok())
                    .and_then(|status| status.head.as_ref());
                let open_head = open.get(key).and_then(Option::as_ref);
                for head in [entry.head.as_ref(), status_head, open_head]
                    .into_iter()
                    .flatten()
                {
                    if self.headlines.contains_key(head) {
                        cached.insert(head.clone());
                    }
                }
            }
            let keys = repository.keys.clone();
            let seed_key_for_heads = repository.seed_key.clone();
            let extra = self.missing_heads(open, |key| {
                key == seed_key_for_heads || keys.iter().any(|candidate| candidate == key)
            });
            self.requested_heads.extend(extra.iter().cloned());
            if let Some(repository) = self.repositories.get_mut(index) {
                repository.seed_key = seed_key.clone();
                repository.seed = seed.clone();
            }
            let request = self.start(Target::Listing(seed_key), false, now);
            work.push(Work::Listing {
                request,
                seed,
                cached,
                extra,
            });
            budget -= 1;
        }
        if budget == 0 {
            return work;
        }
        let mut statuses: Vec<(Option<Instant>, String, PathBuf, Option<String>)> = Vec::new();
        for repository in &self.repositories {
            for (entry, key) in repository.entries.iter().zip(&repository.keys) {
                if open.contains_key(key)
                    || entry.bare
                    || entry.prunable.is_some()
                    || in_flight.contains(&Target::Status(key.clone()))
                    || statuses.iter().any(|(_, candidate, _, _)| candidate == key)
                {
                    continue;
                }
                let refreshed = self.statuses.get(key).map(|status| status.refreshed);
                if refreshed.is_some_and(|refreshed| {
                    now.saturating_duration_since(refreshed) < STATUS_INTERVAL
                        && !self.requested_after(refreshed)
                }) {
                    continue;
                }
                let head = self
                    .statuses
                    .get(key)
                    .and_then(|status| status.result.as_ref().ok())
                    .and_then(|status| status.head.clone())
                    .or_else(|| entry.head.clone())
                    .filter(|head| self.headlines.contains_key(head));
                statuses.push((refreshed, key.clone(), entry.path.clone(), head));
            }
        }
        // Never-read worktrees first, then the oldest.
        statuses.sort_by_key(|(refreshed, _, _, _)| *refreshed);
        for (_, key, path, cached_head) in statuses.into_iter().take(budget) {
            let request = self.start(Target::Status(key), false, now);
            work.push(Work::Status {
                request,
                path,
                cached_head,
            });
        }
        work
    }

    fn needs_discovery(&self, key: &str, in_flight: &HashSet<Target>, now: Instant) -> bool {
        let covered = self
            .repositories
            .iter()
            .any(|repository| repository.covers(key));
        // A failed listing waits like a due one, unless an action asked again.
        let backed_off = self.failed.get(key).is_some_and(|(at, _)| {
            now.saturating_duration_since(*at) < LISTING_INTERVAL && !self.requested_after(*at)
        });
        !covered && !backed_off && !in_flight.contains(&Target::Listing(key.to_owned()))
    }

    fn details_work(&mut self, path: &Path, now: Instant) -> Option<Work> {
        let key = path_key(path);
        let target = Target::Details(key.clone());
        if self.jobs.len() >= MAX_JOBS
            || self.jobs.values().any(|job| job.target == target)
            || self.details.get(&key).is_some_and(|entry| {
                now.saturating_duration_since(entry.refreshed) < STATUS_INTERVAL
                    && !self.requested_after(entry.refreshed)
            })
        {
            return None;
        }
        let request = self.start(target, false, now);
        Some(Work::Details {
            request,
            path: path.to_path_buf(),
        })
    }

    /// Heads of open worktrees in one repository that have no headline yet
    /// and were not asked for before.
    fn missing_heads(
        &self,
        open: &HashMap<String, Option<String>>,
        belongs: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let mut heads: Vec<String> = open
            .iter()
            .filter(|(key, _)| belongs(key))
            .filter_map(|(_, head)| head.clone())
            .filter(|head| {
                !self.headlines.contains_key(head) && !self.requested_heads.contains(head)
            })
            .collect();
        heads.sort_unstable();
        heads.dedup();
        heads
    }

    pub(in crate::app) fn finish(&mut self, result: OverviewResult) {
        match result {
            OverviewResult::Details { request, result } => {
                let Some(Job {
                    target: Target::Details(key),
                    started,
                    ..
                }) = self.jobs.remove(&request)
                else {
                    return;
                };
                if self
                    .details
                    .get(&key)
                    .is_some_and(|entry| entry.request > request)
                {
                    return;
                }
                self.details.insert(
                    key,
                    DetailsEntry {
                        result,
                        refreshed: started,
                        request,
                    },
                );
            }
            OverviewResult::Listing { request, result } => {
                let Some(Job {
                    target: Target::Listing(seed_key),
                    started,
                    ..
                }) = self.jobs.remove(&request)
                else {
                    return;
                };
                match result {
                    Ok(listing) => self.apply_listing(request, seed_key, started, listing),
                    Err(error) => {
                        if let Some(repository) = self
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.covers(&seed_key))
                        {
                            if repository.request < request {
                                // Keep showing the last good listing.
                                repository.error = Some(error);
                                repository.refreshed = started;
                                repository.request = request;
                            }
                        } else {
                            self.failed.insert(seed_key, (started, error));
                        }
                    }
                }
            }
            OverviewResult::Status { request, result } => {
                let Some(Job {
                    target: Target::Status(key),
                    started,
                    ..
                }) = self.jobs.remove(&request)
                else {
                    return;
                };
                if self
                    .statuses
                    .get(&key)
                    .is_some_and(|status| status.request > request)
                {
                    return;
                }
                let result = result.map(|read| {
                    if let Some(headline) = read.headline {
                        self.remember_headline(headline);
                    }
                    read.status
                });
                self.statuses.insert(
                    key,
                    StatusEntry {
                        result,
                        refreshed: started,
                        request,
                    },
                );
            }
        }
    }

    fn apply_listing(
        &mut self,
        request: u64,
        seed_key: String,
        started: Instant,
        listing: Listing,
    ) {
        for headline in listing.headlines {
            self.remember_headline(headline);
        }
        let keys: Vec<String> = listing
            .entries
            .iter()
            .map(|entry| path_key(&entry.path))
            .collect();
        let Some(key) = keys.first().cloned() else {
            self.failed
                .insert(seed_key, (started, "Git listed no worktrees".into()));
            return;
        };
        if self
            .repositories
            .iter()
            .any(|repository| repository.key == key && repository.request > request)
        {
            return;
        }
        let seed = self
            .repositories
            .iter()
            .find(|repository| repository.seed_key == seed_key)
            .map(|repository| repository.seed.clone())
            .unwrap_or_else(|| {
                listing
                    .entries
                    .iter()
                    .zip(&keys)
                    .find(|(_, candidate)| **candidate == seed_key)
                    .map(|(entry, _)| entry.path.clone())
                    .unwrap_or_else(|| listing.entries[0].path.clone())
            });
        self.failed.remove(&seed_key);
        // A seed that moved to another repository leaves its old group.
        self.repositories
            .retain(|repository| repository.key != key && repository.seed_key != seed_key);
        self.repositories.push(Repository {
            key,
            seed,
            seed_key,
            entries: listing.entries,
            keys,
            refreshed: started,
            request,
            error: None,
        });
    }

    /// Rows in project tab order: each repository's main worktree first, its
    /// linked worktrees beneath by name, then projects whose listing is pending.
    fn rows(&self, projects: &[OpenProject<'_>]) -> Vec<Row> {
        let mut groups: Vec<(usize, Vec<Row>)> = Vec::new();
        let mut covered = vec![false; projects.len()];
        for repository in &self.repositories {
            let mut rows = Vec::new();
            let mut order = usize::MAX;
            for (position, (entry, key)) in
                repository.entries.iter().zip(&repository.keys).enumerate()
            {
                let matching: Vec<usize> = projects
                    .iter()
                    .enumerate()
                    .filter(|(_, project)| project.maps_to(key))
                    .map(|(index, _)| index)
                    .collect();
                for &index in &matching {
                    covered[index] = true;
                    order = order.min(projects[index].index);
                }
                let preferred = matching
                    .iter()
                    .copied()
                    .find(|&index| projects[index].current)
                    .or_else(|| {
                        matching
                            .iter()
                            .copied()
                            .find(|&index| projects[index].root_key == *key)
                    })
                    .or_else(|| matching.first().copied());
                let kind = if entry.bare {
                    RowKind::Bare
                } else if let Some(reason) = &entry.prunable {
                    RowKind::Missing(reason.clone())
                } else {
                    RowKind::Worktree
                };
                let status = if kind == RowKind::Worktree {
                    preferred
                        .and_then(|index| projects[index].status())
                        .or_else(|| self.statuses.get(key).map(|status| status.result.clone()))
                } else {
                    None
                };
                let known = status.as_ref().and_then(|status| status.as_ref().ok());
                let (branch, head) = match known {
                    Some(status) => (status.tracking.name.clone(), status.head.clone()),
                    None => (entry.branch.clone(), entry.head.clone()),
                };
                rows.push(Row {
                    key: key.clone(),
                    path: entry.path.clone(),
                    name: folder_name(&entry.path),
                    linked: position > 0,
                    project: preferred.map(|index| projects[index].index),
                    current: matching.iter().any(|&index| projects[index].current),
                    kind,
                    branch,
                    headline: self.headline(head.as_ref()),
                    head,
                    locked: entry.locked.clone(),
                    status,
                    details: self.details.get(key).map(|entry| entry.result.clone()),
                    listing_error: (position == 0).then(|| repository.error.clone()).flatten(),
                });
            }
            if let Some(linked) = rows.get_mut(1..) {
                linked.sort_by_cached_key(|row| row.name.to_lowercase());
            }
            groups.push((order, rows));
        }
        let mut standalone: Vec<(String, usize, Row)> = Vec::new();
        for (position, project) in projects.iter().enumerate() {
            let (false, Some(repository), Some(key)) = (
                covered[position],
                project.repository,
                project.repository_key.as_ref(),
            ) else {
                continue;
            };
            // Several projects inside one repository share its row.
            if let Some((_, _, row)) = standalone
                .iter_mut()
                .find(|(candidate, _, _)| candidate == key)
            {
                if project.current {
                    row.current = true;
                    row.project = Some(project.index);
                    row.status = project.status();
                }
                continue;
            }
            let status = project.status();
            let head = status
                .as_ref()
                .and_then(|status| status.as_ref().ok())
                .and_then(|status| status.head.clone());
            standalone.push((
                key.clone(),
                project.index,
                Row {
                    key: key.clone(),
                    path: repository.to_path_buf(),
                    name: folder_name(repository),
                    linked: false,
                    project: Some(project.index),
                    current: project.current,
                    kind: RowKind::Worktree,
                    branch: status
                        .as_ref()
                        .and_then(|status| status.as_ref().ok())
                        .and_then(|status| status.tracking.name.clone()),
                    headline: self.headline(head.as_ref()),
                    head,
                    locked: None,
                    status,
                    details: self.details.get(key).map(|entry| entry.result.clone()),
                    listing_error: None,
                },
            ));
        }
        groups.extend(
            standalone
                .into_iter()
                .map(|(_, order, row)| (order, vec![row])),
        );
        groups.sort_by_key(|(order, _)| *order);
        groups.into_iter().flat_map(|(_, rows)| rows).collect()
    }
}

fn overview_now(ui: &egui::Ui) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    // Snapshot tests inject a fixed clock; production uses wall-clock time.
    #[cfg(test)]
    let now = ui
        .data(|data| data.get_temp::<i64>(egui::Id::new("git-history-test-clock")))
        .unwrap_or(now);
    #[cfg(not(test))]
    let _ = ui;
    now
}

/// Upstream divergence in the row: nothing when in sync or untracked.
fn sync_marker(status: &git::WorktreeStatus) -> Option<(String, Color32)> {
    let tracking = &status.tracking;
    tracking.upstream.as_ref()?;
    if !tracking.divergence_known {
        return Some(("↑? ↓?".into(), ORANGE));
    }
    let mut parts = Vec::new();
    if tracking.ahead > 0 {
        parts.push(format!("↑{}", compact_number(tracking.ahead as u64)));
    }
    if tracking.behind > 0 {
        parts.push(format!("↓{}", compact_number(tracking.behind as u64)));
    }
    (!parts.is_empty()).then(|| (parts.join(" "), MUTED))
}

fn tooltip_note(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) {
    ui.label(RichText::new(text).size(11.0).color(color));
}

fn file_count(count: usize) -> String {
    format!("{count} {}", if count == 1 { "file" } else { "files" })
}

fn repository_tooltip(ui: &mut egui::Ui, row: &Row, now: i64) {
    ui.set_width((ui.ctx().content_rect().width() - 32.0).clamp(160.0, 400.0));
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    ui.spacing_mut().item_spacing = Vec2::new(10.0, 5.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&row.name).size(15.0).strong().color(TEXT));
        tooltip_note(
            ui,
            if row.linked { "Worktree" } else { "Repository" },
            MUTED,
        );
    });
    tooltip_note(ui, crate::paths::display(&row.path), MUTED);
    ui.add_space(3.0);
    ui.separator();

    let known = row.status.as_ref().and_then(|status| status.as_ref().ok());
    if row.kind != RowKind::Bare {
        ui.horizontal_wrapped(|ui| match (&row.branch, &row.head) {
            (Some(branch), _) => {
                tooltip_note(ui, "Branch", MUTED);
                ui.label(RichText::new(branch).monospace().color(BLUE));
            }
            (None, Some(head)) => {
                tooltip_note(ui, "Detached HEAD", ORANGE);
                ui.label(RichText::new(short_hash(head)).monospace().color(BLUE));
            }
            _ => {}
        });
        if let Some(status) = known {
            let tracking = &status.tracking;
            match &tracking.upstream {
                Some(upstream) => {
                    tooltip_note(ui, format!("Tracking {upstream}"), MUTED);
                    if !tracking.divergence_known {
                        tooltip_note(ui, "Upstream status unavailable · fetch to refresh", ORANGE);
                    } else if tracking.ahead == 0 && tracking.behind == 0 {
                        tooltip_note(ui, "Up to date", GREEN);
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("↑ {} to push", tracking.ahead)).color(BLUE),
                            );
                            ui.label(
                                RichText::new(format!("↓ {} to pull", tracking.behind))
                                    .color(ORANGE),
                            );
                        });
                    }
                }
                None if row.branch.is_some() => tooltip_note(ui, "No upstream", MUTED),
                None => {}
            }
        }
    }

    if row.openable() {
        ui.add_space(5.0);
        tooltip_note(ui, "Uncommitted changes", MUTED);
        let details = row
            .details
            .as_ref()
            .and_then(|details| details.as_ref().ok());
        let count = details
            .map(|details| details.files)
            .or_else(|| known.map(|status| status.changes));
        ui.horizontal_wrapped(|ui| {
            if let Some(count) = count {
                ui.label(
                    RichText::new(format!("{} changed", file_count(count)))
                        .strong()
                        .color(TEXT),
                );
            }
            if let Some(details) = details {
                ui.label(
                    RichText::new(format!("+{}", details.added))
                        .monospace()
                        .size(16.0)
                        .strong()
                        .color(GREEN),
                );
                ui.label(
                    RichText::new(format!("−{}", details.removed))
                        .monospace()
                        .size(16.0)
                        .strong()
                        .color(RED),
                );
            }
        });
        if details.is_some() {
            tooltip_note(ui, "Staged, unstaged & untracked · saved files", MUTED);
        }
        if let Some(status) = known
            && status.conflicts > 0
        {
            ui.label(
                RichText::new(format!(
                    "{} {}",
                    status.conflicts,
                    if status.conflicts == 1 {
                        "merge conflict"
                    } else {
                        "merge conflicts"
                    }
                ))
                .strong()
                .color(RED),
            );
        }
        match &row.details {
            Some(Ok(details)) => {
                if details.binary > 0 {
                    tooltip_note(
                        ui,
                        format!(
                            "{} binary {} · no line counts",
                            details.binary,
                            if details.binary == 1 { "file" } else { "files" }
                        ),
                        MUTED,
                    );
                }
                if details.unknown > 0 {
                    tooltip_note(
                        ui,
                        format!(
                            "Partial line counts · {} unavailable",
                            file_count(details.unknown)
                        ),
                        ORANGE,
                    );
                }
            }
            Some(Err(error)) => {
                tooltip_note(ui, "Line counts unavailable", ORANGE);
                tooltip_note(ui, error, MUTED);
            }
            None => tooltip_note(ui, "Loading line counts…", MUTED),
        }
        match &row.status {
            Some(Err(error)) => {
                tooltip_note(ui, "Status unavailable", ORANGE);
                tooltip_note(ui, error, MUTED);
            }
            None if details.is_none() => tooltip_note(ui, "Reading status…", MUTED),
            _ => {}
        }
    }

    if let Some(headline) = &row.headline {
        ui.add_space(4.0);
        ui.separator();
        tooltip_note(ui, "Last commit", MUTED);
        ui.label(RichText::new(&headline.subject).strong().color(TEXT));
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(short_hash(&headline.hash))
                    .monospace()
                    .size(11.0)
                    .color(BLUE),
            );
            tooltip_note(
                ui,
                format!(
                    "{} · {}",
                    headline.author,
                    relative_time(headline.timestamp, now)
                ),
                MUTED,
            );
        });
    } else if row.kind == RowKind::Worktree && row.head.is_none() && row.branch.is_some() {
        tooltip_note(ui, "No commits yet", MUTED);
    }
    match &row.kind {
        RowKind::Bare => tooltip_note(ui, "Bare repository without a working tree", MUTED),
        RowKind::Missing(reason) => {
            tooltip_note(ui, "Worktree directory is missing", RED);
            tooltip_note(
                ui,
                if reason.is_empty() {
                    "git worktree prune removes it"
                } else {
                    reason
                },
                MUTED,
            );
        }
        RowKind::Worktree => {}
    }
    if let Some(reason) = &row.locked {
        tooltip_note(
            ui,
            if reason.is_empty() {
                "Locked".into()
            } else {
                format!("Locked: {reason}")
            },
            ORANGE,
        );
    }
    if let Some(error) = &row.listing_error {
        tooltip_note(ui, "Worktree list unavailable", ORANGE);
        tooltip_note(ui, error, MUTED);
    }
    ui.add_space(3.0);
    ui.separator();
    let (action, color) = if row.current {
        ("Current project", ACCENT)
    } else if row.project.is_some() {
        ("Open project · click to switch", MUTED)
    } else if row.openable() {
        ("Click to open as project", MUTED)
    } else {
        ("Cannot be opened as a project", MUTED)
    };
    tooltip_note(ui, action, color);
}

impl VEditApp {
    pub(in crate::app) fn worktree_selector_ui(&mut self, ui: &mut egui::Ui) -> Option<egui::Id> {
        let rows = self.repository_rows();
        if rows.len() < 2 {
            return None;
        }
        let current = rows.iter().find(|row| row.current)?;
        let title = if current.linked {
            current.name.as_str()
        } else {
            "Main worktree"
        };
        let button = egui::Button::new(RichText::new(format!("{title} ▾")).size(12.5))
            .truncate()
            .min_size(Vec2::new(ui.available_width(), 26.0));
        let mut chosen = None;
        let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
            ui.set_width(300.0);
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for row in &rows {
                        let mut name = if row.linked {
                            row.name.clone()
                        } else {
                            format!("Main worktree · {}", row.name)
                        };
                        if !row.openable() {
                            name.push_str(&format!(" · {}", row.badge()));
                        }
                        let response = ui.add_enabled(
                            row.openable(),
                            egui::Button::new("")
                                .selected(row.current)
                                .min_size(Vec2::new(ui.available_width(), 46.0)),
                        );
                        let rect = response.rect;
                        if ui.is_rect_visible(rect) {
                            let painter =
                                ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
                            for (text, y, size, color) in [
                                (
                                    name.as_str(),
                                    6.0,
                                    12.5,
                                    if row.openable() { TEXT } else { MUTED },
                                ),
                                (row.summary().as_str(), 26.0, 10.5, MUTED),
                            ] {
                                let galley = truncated_text(
                                    ui,
                                    text,
                                    FontId::proportional(size),
                                    color,
                                    rect.width() - 16.0,
                                );
                                painter.galley(rect.left_top() + Vec2::new(8.0, y), galley, color);
                            }
                        }
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                row.openable(),
                                row.current,
                                row.label(),
                            )
                        });
                        let response = self.worktree_picker_hover(ui, response, row);
                        if response.clicked() {
                            if !row.current {
                                chosen = Some((row.project, row.path.clone()));
                            }
                            ui.close();
                        }
                    }
                });
        });
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Select worktree")
        });
        git_popup_focus(&response, egui::Popup::default_response_id(&response));
        let id = response.id;
        self.worktree_picker_hover(ui, response, current);
        if let Some((project, path)) = chosen {
            self.select_worktree(project, path);
        }
        ui.add_space(4.0);
        Some(id)
    }

    pub(in crate::app) fn worktree_picker_hover(
        &mut self,
        ui: &egui::Ui,
        response: egui::Response,
        row: &Row,
    ) -> egui::Response {
        let mut visible = false;
        let response = response.on_hover_details_ui(|ui| {
            visible = true;
            repository_tooltip(ui, row, overview_now(ui));
            row.copy_details(overview_now(ui))
        });
        if visible
            && self.git_polling_active()
            && row.openable()
            && self.git_busy.is_none()
            && let Some(work) = self.repositories.details_work(&row.path, Instant::now())
        {
            self.worker(move || WorkerResult::RepositoryOverview(work.run()));
            ui.ctx().request_repaint();
        }
        response
    }

    fn open_projects_for_overview(&self) -> Vec<OpenProject<'_>> {
        self.projects
            .iter()
            .enumerate()
            .map(|(index, project)| {
                let summary = project.git_summary.as_ref();
                OpenProject {
                    index,
                    current: index == self.active_project,
                    root_key: path_key(&project.root),
                    repository_key: summary.map(|summary| path_key(&summary.repository)),
                    repository: summary.map(|summary| summary.repository.as_path()),
                    status: summary.map(|summary| &summary.status),
                    error: project.git_error.as_deref(),
                }
            })
            .collect()
    }

    pub(in crate::app) fn repository_rows(&self) -> Vec<Row> {
        let projects = self.open_projects_for_overview();
        let Some(current) = projects.iter().find(|project| project.current) else {
            return Vec::new();
        };
        let key = current.repository_key.as_ref().unwrap_or(&current.root_key);
        let repository = self
            .repositories
            .repositories
            .iter()
            .find(|repository| repository.covers(key));
        self.repositories
            .rows(&projects)
            .into_iter()
            .filter(|row| {
                repository.map_or(row.key == *key, |repository| {
                    repository.keys.contains(&row.key)
                })
            })
            .collect()
    }

    /// The active project's first history page usually has its HEAD commit;
    /// reuse it instead of asking Git for that headline.
    fn fill_project_headlines(&mut self) {
        let Some(project) = self.projects.get(self.active_project) else {
            return;
        };
        let Some(head) = project
            .git_summary
            .as_ref()
            .and_then(|summary| summary.status.head.as_ref())
        else {
            return;
        };
        if self.repositories.headlines.contains_key(head) {
            return;
        }
        if let Some(commit) = project
            .recent_commits
            .iter()
            .find(|commit| commit.hash == *head)
        {
            let headline = git::CommitHeadline {
                hash: commit.hash.clone(),
                subject: commit.subject.clone(),
                author: commit.author.clone(),
                timestamp: commit.timestamp,
            };
            self.repositories.remember_headline(headline);
        }
    }

    /// Discovery runs while Source control or the project picker is visible.
    pub(in crate::app) fn refresh_repository_overview(&mut self) {
        if !(self.workspace_picker_open() || self.sidebar_view_visible(SidebarView::SourceControl))
            || !self.git_polling_active()
            || self.git_busy.is_some()
        {
            return;
        }
        self.fill_project_headlines();
        let mut seeds: Vec<(String, PathBuf)> = Vec::new();
        let mut open: HashMap<String, Option<String>> = HashMap::new();
        for project in self.projects.iter().filter(|p| !p.is_other_files()) {
            open.entry(path_key(&project.root)).or_default();
            if let Some(summary) = &project.git_summary {
                let key = path_key(&summary.repository);
                if let Some(head) = &summary.status.head {
                    open.insert(key.clone(), Some(head.clone()));
                } else {
                    open.entry(key.clone()).or_default();
                }
                if !seeds.iter().any(|(candidate, _)| *candidate == key) {
                    seeds.push((key, summary.repository.clone()));
                }
            }
        }
        for work in self.repositories.plan(&seeds, &open, Instant::now()) {
            self.worker(move || WorkerResult::RepositoryOverview(work.run()));
        }
    }
}

#[cfg(test)]
#[path = "git_repositories_tests.rs"]
mod tests;
