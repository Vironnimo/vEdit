//! Repositories overview at the top of Source control.
//!
//! Lists every repository behind the open projects, grouped by its main
//! worktree, with all of its worktrees beneath, including ones that are not
//! open as projects. Open projects reuse their own status refreshes. The
//! overview itself starts one `worktree list` per repository and one status
//! read per worktree that is not open, only while Source control is visible.
//! Request IDs keep a late worker from replacing a newer result.

use super::*;
use crate::palette::BORDER_STRONG;
use std::collections::HashSet;

/// Worktrees added or removed outside vEdit appear within this interval.
const LISTING_INTERVAL: Duration = Duration::from_secs(15);
/// A folded overview shows only its header, so membership can wait longer.
const COLLAPSED_LISTING_INTERVAL: Duration = Duration::from_secs(60);
/// Worktrees that are not open refresh at half the inactive project cadence.
const STATUS_INTERVAL: Duration = Duration::from_secs(30);
const MAX_JOBS: usize = 2;
const MAX_HEADLINES: usize = 512;
const VISIBLE_ROWS: usize = 6;
const HEADER_HEIGHT: f32 = 22.0;
const HEADER_ID: &str = "git-repositories-header";

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

fn row_id(key: &str) -> egui::Id {
    egui::Id::new(("git-repositories-row", key))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Target {
    /// Keyed by the seed path the listing runs in.
    Listing(String),
    /// Keyed by the worktree path.
    Status(String),
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

/// A deferred click, applied after the editor pass like project tab switches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum RepositoryAction {
    Switch(PathBuf),
    Open(PathBuf),
}

#[derive(Default)]
pub(in crate::app) struct RepositoryOverview {
    pub(in crate::app) collapsed: bool,
    repositories: Vec<Repository>,
    /// Worktrees that are not open as projects, by path key.
    statuses: HashMap<String, StatusEntry>,
    headlines: HashMap<String, git::CommitHeadline>,
    headline_order: VecDeque<String>,
    /// Heads already asked for once, so a missing object cannot loop.
    requested_heads: HashSet<String>,
    failed: HashMap<String, (Instant, String)>,
    jobs: HashMap<u64, Job>,
    next_request: u64,
    refresh_requested: Option<Instant>,
    pending: Option<RepositoryAction>,
    /// Keyboard rows of the last rendered frame.
    rows: Vec<egui::Id>,
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
struct Row {
    key: String,
    path: PathBuf,
    name: String,
    linked: bool,
    project: Option<usize>,
    current: bool,
    kind: RowKind,
    branch: Option<String>,
    head: Option<String>,
    locked: Option<String>,
    status: Option<Result<git::WorktreeStatus, String>>,
    headline: Option<git::CommitHeadline>,
    listing_error: Option<String>,
}

impl Row {
    fn openable(&self) -> bool {
        self.kind == RowKind::Worktree
    }

    fn label(&self) -> String {
        format!(
            "{} {}",
            if self.linked {
                "Worktree"
            } else {
                "Repository"
            },
            self.name
        )
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
        let interval = if self.collapsed {
            COLLAPSED_LISTING_INTERVAL
        } else {
            LISTING_INTERVAL
        };
        let mut due: Vec<(Instant, usize)> = self
            .repositories
            .iter()
            .enumerate()
            .filter(|(_, repository)| {
                !in_flight.contains(&Target::Listing(repository.seed_key.clone()))
                    && (now.saturating_duration_since(repository.refreshed) >= interval
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
        if self.collapsed || budget == 0 {
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

/// Compact age beside a subject: "now", "5m", "3h", "2d", "4mo", "1y".
fn compact_age(timestamp: i64, now: i64) -> String {
    if timestamp <= 0 {
        return String::new();
    }
    let seconds = now.saturating_sub(timestamp).max(0);
    match seconds {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        86_400..2_592_000 => format!("{}d", seconds / 86_400),
        2_592_000..31_536_000 => format!("{}mo", seconds / 2_592_000),
        _ => format!("{}y", seconds / 31_536_000),
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

fn tooltip_text(row: &Row, now: i64) -> String {
    let mut lines = vec![crate::paths::display(&row.path)];
    match &row.kind {
        RowKind::Bare => lines.push("Bare repository without a working tree".into()),
        RowKind::Missing(reason) if reason.is_empty() => {
            lines.push("Worktree directory is missing · git worktree prune removes it".into())
        }
        RowKind::Missing(reason) => lines.push(format!("Worktree directory is missing: {reason}")),
        RowKind::Worktree => {}
    }
    let known = row.status.as_ref().and_then(|status| status.as_ref().ok());
    let branch = match (&row.branch, &row.head) {
        (Some(branch), _) => format!("Branch {branch}"),
        (None, Some(head)) => format!("Detached at {}", short_hash(head)),
        (None, None) => String::new(),
    };
    if !branch.is_empty() && row.kind != RowKind::Bare {
        let tracking = known.map(|status| &status.tracking);
        let upstream = match tracking {
            Some(tracking) => match &tracking.upstream {
                Some(upstream) if !tracking.divergence_known => {
                    format!("tracking {upstream} · status unavailable, fetch to refresh")
                }
                Some(upstream) if tracking.ahead == 0 && tracking.behind == 0 => {
                    format!("tracking {upstream} · up to date")
                }
                Some(upstream) => format!(
                    "tracking {upstream} · {} to push · {} to pull",
                    tracking.ahead, tracking.behind
                ),
                None if row.branch.is_some() => "no upstream".into(),
                None => String::new(),
            },
            None => String::new(),
        };
        lines.push(if upstream.is_empty() {
            branch
        } else {
            format!("{branch} · {upstream}")
        });
    }
    match &row.status {
        Some(Ok(status)) => {
            let mut line = match status.changes {
                0 => "No uncommitted changes".to_owned(),
                1 => "1 uncommitted change".to_owned(),
                count => format!("{count} uncommitted changes"),
            };
            if status.conflicts > 0 {
                line.push_str(&format!(
                    " · {} {}",
                    status.conflicts,
                    if status.conflicts == 1 {
                        "conflict"
                    } else {
                        "conflicts"
                    }
                ));
            }
            lines.push(line);
        }
        Some(Err(error)) => lines.push(format!("Status unavailable: {error}")),
        None if row.kind == RowKind::Worktree => lines.push("Reading status…".into()),
        None => {}
    }
    if let Some(headline) = &row.headline {
        lines.push(format!(
            "Last commit {} · {}",
            short_hash(&headline.hash),
            headline.subject
        ));
        lines.push(format!(
            "{} · {}",
            headline.author,
            relative_time(headline.timestamp, now)
        ));
    } else if row.kind == RowKind::Worktree && row.head.is_none() && row.branch.is_some() {
        lines.push("No commits yet".into());
    }
    if let Some(reason) = &row.locked {
        lines.push(if reason.is_empty() {
            "Locked".into()
        } else {
            format!("Locked: {reason}")
        });
    }
    if let Some(error) = &row.listing_error {
        lines.push(format!("Worktree list unavailable: {error}"));
    }
    lines.push(
        if row.current {
            "Current project"
        } else if row.project.is_some() {
            "Open project · click to switch"
        } else if row.openable() {
            "Click to open as project"
        } else {
            "Cannot be opened as a project"
        }
        .into(),
    );
    lines.join("\n")
}

fn paint_chevron(painter: &egui::Painter, center: egui::Pos2, open: bool) {
    let points = if open {
        vec![
            center + Vec2::new(-3.0, -1.5),
            center + Vec2::new(0.0, 1.5),
            center + Vec2::new(3.0, -1.5),
        ]
    } else {
        vec![
            center + Vec2::new(-1.5, -3.0),
            center + Vec2::new(1.5, 0.0),
            center + Vec2::new(-1.5, 3.0),
        ]
    };
    painter.add(egui::Shape::line(points, Stroke::new(1.3, MUTED)));
}

fn header_row(ui: &mut egui::Ui, rows: &[Row], collapsed: bool) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), HEADER_HEIGHT),
        egui::Sense::hover(),
    );
    let response = ui.interact(rect, egui::Id::new(HEADER_ID), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::CollapsingHeader,
            ui.is_enabled(),
            !collapsed,
            "Repositories",
        )
    });
    if ui.is_rect_visible(rect) {
        if response.hovered() || crate::design::keyboard_focus(&response) {
            ui.painter().rect_filled(rect, 4.0, HOVER);
        }
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let repositories = rows.iter().filter(|row| !row.linked).count();
        let worktrees = rows.iter().filter(|row| row.kind != RowKind::Bare).count();
        let conflicts: usize = rows
            .iter()
            .filter_map(|row| row.status.as_ref()?.as_ref().ok())
            .map(|status| status.conflicts)
            .sum();
        let plural = |count: usize, one: &str, many: &str| {
            format!("{count} {}", if count == 1 { one } else { many })
        };
        let mut summary = plural(worktrees, "worktree", "worktrees");
        if repositories > 1 {
            summary = format!(
                "{} · {summary}",
                plural(repositories, "repository", "repositories")
            );
        }
        let (summary, color) = if conflicts > 0 && collapsed {
            (format!("Conflicts · {summary}"), RED)
        } else {
            (summary, MUTED)
        };
        let chevron = egui::pos2(rect.right() - 10.0, rect.center().y);
        paint_chevron(&painter, chevron, !collapsed);
        let summary = painter.layout_no_wrap(summary, FontId::proportional(11.0), color);
        let summary_left = chevron.x - 10.0 - summary.size().x;
        painter.galley(
            egui::pos2(summary_left, rect.center().y - summary.size().y * 0.5),
            summary,
            color,
        );
        let title = truncated_text(
            ui,
            "Repositories",
            FontId::proportional(11.5),
            MUTED,
            (summary_left - rect.left() - SECTION_TEXT_INSET - 8.0).max(1.0),
        );
        painter.galley(
            egui::pos2(
                rect.left() + SECTION_TEXT_INSET,
                rect.center().y - title.size().y * 0.5,
            ),
            title,
            MUTED,
        );
    }
    git_row_keyboard(ui, &response);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if collapsed {
            "Show repositories and worktrees"
        } else {
            "Hide repositories and worktrees"
        })
}

fn repository_row(ui: &mut egui::Ui, row: &Row, now: i64) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(1.0), TREE_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let response = ui.interact(rect, row_id(&row.key), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            row.current,
            row.label(),
        )
    });
    if ui.is_rect_visible(rect) {
        let focused = crate::design::keyboard_focus(&response);
        if response.hovered() || focused {
            ui.painter().rect_filled(rect, 4.0, HOVER);
        } else if row.current {
            ui.painter().rect_filled(rect, 4.0, SURFACE);
        }
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let center_y = rect.center().y;
        let mut left = rect.left() + SECTION_TEXT_INSET + 4.0;
        if row.linked {
            // A quiet branch guide instead of a folder icon.
            let guide = left + 4.0;
            painter.line_segment(
                [
                    egui::pos2(guide, rect.top() + 2.0),
                    egui::pos2(guide, center_y),
                ],
                Stroke::new(1.0, BORDER_STRONG),
            );
            painter.line_segment(
                [
                    egui::pos2(guide, center_y),
                    egui::pos2(guide + 6.0, center_y),
                ],
                Stroke::new(1.0, BORDER_STRONG),
            );
            left += 14.0;
        }
        if row.current {
            painter.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 2.0, center_y),
                    Vec2::new(2.0, 12.0),
                ),
                1.0,
                MUTED,
            );
        }
        // Right side, from the edge inward: attention badge, divergence, age.
        let mut right = rect.right() - 6.0;
        let known = row.status.as_ref().and_then(|status| status.as_ref().ok());
        let badge = match (&row.kind, &row.status) {
            (RowKind::Missing(_), _) => Some(("missing".to_owned(), ORANGE, false)),
            (RowKind::Bare, _) => Some(("bare".to_owned(), MUTED, false)),
            (_, Some(Err(_))) => Some(("?".to_owned(), ORANGE, true)),
            (_, Some(Ok(status))) if status.conflicts > 0 => Some((
                format!("!{}", compact_number(status.conflicts as u64)),
                RED,
                true,
            )),
            (_, Some(Ok(status))) if status.changes > 0 => {
                Some((compact_number(status.changes as u64), MUTED, true))
            }
            _ => None,
        };
        if let Some((text, color, pill)) = badge {
            let galley = painter.layout_no_wrap(text, FontId::proportional(10.5), color);
            let size = galley.size();
            let width = if pill { size.x + 10.0 } else { size.x };
            let pill_rect = egui::Rect::from_min_size(
                egui::pos2(right - width, center_y - 8.0),
                Vec2::new(width, 16.0),
            );
            if pill {
                painter.rect_filled(
                    pill_rect,
                    8.0,
                    if row.current || response.hovered() || focused {
                        BORDER_STRONG
                    } else {
                        SURFACE
                    },
                );
            }
            painter.galley(
                egui::pos2(pill_rect.center().x - size.x * 0.5, center_y - size.y * 0.5),
                galley,
                color,
            );
            right -= width + 6.0;
        }
        if let Some((text, color)) = known.and_then(sync_marker) {
            let galley = painter.layout_no_wrap(text, FontId::proportional(11.0), color);
            right -= galley.size().x;
            painter.galley(
                egui::pos2(right, center_y - galley.size().y * 0.5),
                galley,
                color,
            );
            right -= 6.0;
        }
        let age = row
            .headline
            .as_ref()
            .map(|headline| compact_age(headline.timestamp, now))
            .unwrap_or_default();
        let age_galley = (!age.is_empty())
            .then(|| painter.layout_no_wrap(age, FontId::proportional(11.0), MUTED));
        let age_width = age_galley
            .as_ref()
            .map_or(0.0, |galley| galley.size().x + 6.0);
        // Left side: name, branch or short hash, then the latest subject.
        let available = (right - left - age_width).max(1.0);
        // Brightness, not hue: current, then open projects, then the rest.
        let name_color = if row.current || response.hovered() || focused {
            TEXT
        } else if row.project.is_some() {
            TEXT.lerp_to_gamma(MUTED, 0.3)
        } else {
            MUTED
        };
        let name = truncated_text(
            ui,
            &row.name,
            FontId::proportional(12.0),
            name_color,
            available * 0.5,
        );
        let mut x = left;
        x += name.size().x;
        painter.galley(
            egui::pos2(left, center_y - name.size().y * 0.5),
            name,
            name_color,
        );
        let reference = match (&row.kind, &row.branch, &row.head) {
            (RowKind::Bare, _, _) => None,
            (_, Some(branch), _) => Some((branch.clone(), FontId::proportional(11.5))),
            (_, None, Some(head)) => Some((short_hash(head).to_owned(), FontId::monospace(11.0))),
            _ => None,
        };
        if let Some((text, font)) = reference {
            let width = (right - age_width - x - 6.0) * 0.6;
            if width > 16.0 {
                let galley = truncated_text(ui, &text, font, MUTED, width);
                painter.galley(
                    egui::pos2(x + 6.0, center_y - galley.size().y * 0.5),
                    galley.clone(),
                    MUTED,
                );
                x += 6.0 + galley.size().x;
            }
        }
        let subject_width = right - age_width - x - 10.0;
        // A few letters of a subject are noise; narrow panels drop it.
        if let Some(headline) = &row.headline
            && subject_width > 56.0
        {
            let color = MUTED.gamma_multiply(0.75);
            let galley = truncated_text(
                ui,
                &headline.subject,
                FontId::proportional(11.5),
                color,
                subject_width,
            );
            painter.galley(
                egui::pos2(x + 10.0, center_y - galley.size().y * 0.5),
                galley,
                color,
            );
        }
        if let Some(galley) = age_galley {
            painter.galley(
                egui::pos2(right - galley.size().x, center_y - galley.size().y * 0.5),
                galley,
                MUTED,
            );
        }
    }
    git_row_keyboard(ui, &response);
    let response = if row.openable() || row.project.is_some() {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    // Built only while hovered.
    response.on_hover_ui(|ui| {
        ui.set_max_width(440.0);
        ui.label(tooltip_text(row, now));
    })
}

enum RowAction {
    Activate,
    Reveal,
    Copy,
}

impl VEditApp {
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

    fn repository_rows(&self) -> Vec<Row> {
        self.repositories.rows(&self.open_projects_for_overview())
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

    /// Start due overview reads. Runs every frame but only plans while Source
    /// control is visible and no Git action holds the repository.
    pub(in crate::app) fn refresh_repository_overview(&mut self) {
        if !self.sidebar_view_visible(SidebarView::SourceControl) || self.git_busy.is_some() {
            return;
        }
        self.fill_project_headlines();
        let mut seeds: Vec<(String, PathBuf)> = Vec::new();
        let mut open: HashMap<String, Option<String>> = HashMap::new();
        for project in &self.projects {
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

    pub(in crate::app) fn finish_repository_action(&mut self) {
        match self.repositories.pending.take() {
            Some(RepositoryAction::Switch(root)) => {
                if let Some(index) = self
                    .projects
                    .iter()
                    .position(|project| project.root == root)
                {
                    self.switch_project(index);
                    // The source was rendered before this deferred switch.
                    self.ctx.request_repaint();
                }
            }
            Some(RepositoryAction::Open(path)) => {
                self.add_project(path);
                self.ctx.request_repaint();
            }
            None => {}
        }
    }

    pub(in crate::app) fn repository_overview_has_focus(&self) -> bool {
        self.ctx
            .memory(|memory| memory.focused())
            .is_some_and(|focused| self.repositories.rows.contains(&focused))
    }

    /// The overview only appears once there is something to choose between.
    pub(in crate::app) fn repository_overview_ui(&mut self, ui: &mut egui::Ui) {
        let rows = self.repository_rows();
        if rows.len() < 2 {
            self.repositories.rows.clear();
            return;
        }
        let now = overview_now(ui);
        let mut keyboard_rows = vec![egui::Id::new(HEADER_ID)];
        if !self.repositories.collapsed {
            keyboard_rows.extend(rows.iter().map(|row| row_id(&row.key)));
        }
        if let Some(focused) = ui.memory(|memory| memory.focused())
            && ui.is_enabled()
            && !egui::Popup::is_any_open(ui.ctx())
            && let Some(target) = take_git_navigation(
                ui.ctx(),
                &keyboard_rows,
                focused,
                VISIBLE_ROWS,
                self.settings.swap_jk,
            )
        {
            request_git_row_focus(ui.ctx(), target);
        }
        self.repositories.rows = keyboard_rows;
        let mut actions: Vec<(usize, RowAction)> = Vec::new();
        let mut collapsed = self.repositories.collapsed;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let header = header_row(ui, &rows, collapsed);
            let mut open = !collapsed;
            git_folder_key(ui, &header, &mut open);
            if header.clicked() {
                open = !open;
            }
            collapsed = !open;
            if !collapsed {
                egui::ScrollArea::vertical()
                    .id_salt("git-repositories")
                    .max_height(VISIBLE_ROWS as f32 * TREE_ROW_HEIGHT)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (index, row) in rows.iter().enumerate() {
                            let response = repository_row(ui, row, now);
                            if response.clicked() {
                                actions.push((index, RowAction::Activate));
                            }
                            git_context_menu(&response, |ui, focus| {
                                let openable = row.openable();
                                let open = ui
                                    .add_enabled_ui(openable, |ui| {
                                        git_menu_item(
                                            ui,
                                            "Open as project",
                                            "",
                                            if row.project.is_some() {
                                                "Switch to this open project"
                                            } else {
                                                "Add this worktree as a project"
                                            },
                                            focus && openable,
                                        )
                                    })
                                    .inner;
                                if open.clicked() {
                                    actions.push((index, RowAction::Activate));
                                    ui.close();
                                }
                                let exists = !matches!(row.kind, RowKind::Missing(_));
                                let reveal = ui
                                    .add_enabled_ui(exists, |ui| {
                                        git_menu_item(
                                            ui,
                                            "Reveal in Explorer",
                                            "",
                                            "Show this folder in Windows Explorer",
                                            focus && !openable && exists,
                                        )
                                    })
                                    .inner;
                                if reveal.clicked() {
                                    actions.push((index, RowAction::Reveal));
                                    ui.close();
                                }
                                if git_menu_item(
                                    ui,
                                    "Copy path",
                                    "",
                                    "Copy the full folder path",
                                    focus && !openable && !exists,
                                )
                                .clicked()
                                {
                                    actions.push((index, RowAction::Copy));
                                    ui.close();
                                }
                            });
                        }
                    });
            }
            ui.add_space(6.0);
            let (rule, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().line_segment(
                [rule.left_center(), rule.right_center()],
                Stroke::new(1.0, BORDER),
            );
            ui.add_space(6.0);
        });
        if collapsed != self.repositories.collapsed {
            self.repositories.collapsed = collapsed;
            self.session_dirty = true;
        }
        for (index, action) in actions {
            let row = &rows[index];
            match action {
                RowAction::Activate if row.current => {}
                RowAction::Activate => {
                    if let Some(project) = row.project.and_then(|index| self.projects.get(index)) {
                        self.repositories.pending =
                            Some(RepositoryAction::Switch(project.root.clone()));
                    } else if row.openable() {
                        self.repositories.pending = Some(RepositoryAction::Open(row.path.clone()));
                    }
                }
                RowAction::Reveal => self.context_reveal(row.path.clone()),
                RowAction::Copy => self.ctx.copy_text(crate::paths::display(&row.path)),
            }
        }
    }
}

#[cfg(test)]
#[path = "git_repositories_tests.rs"]
mod tests;
