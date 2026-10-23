//! Frozen history traversal, refs, and branch comparisons.

use super::*;

/// Compare HEAD with local main (or master). This reads local objects only;
/// the independent tracking comparison describes the last fetched remote state.
pub fn main_comparison(root: &Path) -> GitResult<Option<BranchComparison>> {
    let Some(base) = local_main_branch(root)? else {
        return Ok(None);
    };
    compare_with_main(root, base)
}

/// [`main_comparison`] for a refresh that has just read the branch refs with
/// [`history_snapshot_after_status`]. The base branch comes from that listing,
/// and a HEAD at the base commit needs no count, so an idle refresh on main
/// launches no Git process here.
pub fn main_comparison_after_history(
    root: &Path,
    refs: &HistoryHead,
) -> GitResult<Option<BranchComparison>> {
    let Some((base, base_commit)) = &refs.main else {
        return Ok(None);
    };
    if refs.head.is_some() && refs.head == *base_commit {
        return Ok(Some(BranchComparison {
            base: (*base).into(),
            ahead: 0,
            behind: 0,
        }));
    }
    compare_with_main(root, base)
}

fn compare_with_main(root: &Path, base: &str) -> GitResult<Option<BranchComparison>> {
    let output = checked_git(
        root,
        &string_args(&[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...refs/heads/{base}"),
            "--",
        ]),
        "compare with main branch",
    )?;
    let counts = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::parse::<usize>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| GitError::new("compare with main branch", "invalid commit counts"))?;
    let [ahead, behind] = counts.as_slice() else {
        return Err(GitError::new(
            "compare with main branch",
            "invalid commit counts",
        ));
    };
    Ok(Some(BranchComparison {
        base: base.into(),
        ahead: *ahead,
        behind: *behind,
    }))
}

/// The local branch that other branches are compared with: `main`, otherwise
/// `master`. Remote-tracking refs are deliberately ignored.
pub(super) fn local_main_branch(root: &Path) -> GitResult<Option<&'static str>> {
    let output = checked_git(
        root,
        &string_args(&[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/main",
            "refs/heads/master",
        ]),
        "find main branch",
    )?;
    let refs = String::from_utf8_lossy(&output.stdout);
    Ok(["main", "master"].into_iter().find(|name| {
        refs.lines()
            .any(|line| line == format!("refs/heads/{name}"))
    }))
}

/// Return up to `limit` commits in topological order across repository branches.
#[cfg(test)]
pub fn recent_log(root: &Path, limit: usize) -> GitResult<Vec<CommitSummary>> {
    let snapshot = history_snapshot(root)?;
    Ok(history_page(root, &snapshot, 0, limit)?.commits)
}

pub fn history_snapshot(root: &Path) -> GitResult<HistorySnapshot> {
    history_snapshot_cached(root, None)
}

pub fn history_snapshot_cached(
    root: &Path,
    previous: Option<&HistorySnapshot>,
) -> GitResult<HistorySnapshot> {
    let head = read_head(root)?;
    let refs = read_history_refs(root)?;
    assemble_history_snapshot(root, previous, head, refs)
}

/// HEAD and local main (or master) as seen by [`history_snapshot_after_status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryHead {
    head: Option<String>,
    /// The base branch name and its commit, when it is a plain commit ref.
    main: Option<(&'static str, Option<String>)>,
}

/// [`history_snapshot_cached`] for a refresh that has just read the status.
/// The HEAD commit reported there is used while the listed refs agree with
/// it; refs that moved after the status read fall back to reading HEAD again,
/// so the decorations are the same as a separate HEAD read would give.
pub fn history_snapshot_after_status(
    root: &Path,
    previous: Option<&HistorySnapshot>,
    branch: Option<&str>,
    status_head: Option<&StatusHead>,
) -> GitResult<(HistorySnapshot, HistoryHead)> {
    let refs = read_history_refs(root)?;
    let checked_out = refs
        .iter()
        .find(|reference| reference.kind == RefKind::Local && reference.checked_out);
    let reported = match (branch, status_head, checked_out) {
        (Some(branch), Some(StatusHead::Commit(hash)), Some(reference)) => {
            (reference.name == branch && reference.commit.as_deref() == Some(hash))
                .then(|| Some(hash.clone()))
        }
        (Some(_), Some(StatusHead::Unborn), None) => Some(None),
        (None, Some(StatusHead::Commit(hash)), None) => Some(Some(hash.clone())),
        _ => None,
    };
    let head = match reported {
        Some(head) => head,
        None => read_head(root)?,
    };
    let main = ["main", "master"].into_iter().find_map(|name| {
        refs.iter()
            .find(|reference| reference.kind == RefKind::Local && reference.name == name)
            .map(|reference| (name, reference.commit.clone()))
    });
    let snapshot = assemble_history_snapshot(root, previous, head.clone(), refs)?;
    Ok((snapshot, HistoryHead { head, main }))
}

/// One local, remote-tracking or tag ref. `commit` is the commit it decorates,
/// or `None` for symbolic refs and refs to other objects.
struct HistoryRef {
    name: String,
    kind: RefKind,
    commit: Option<String>,
    checked_out: bool,
    upstream: Option<String>,
}

fn read_history_refs(root: &Path) -> GitResult<Vec<HistoryRef>> {
    let output = checked_git(
        root,
        &string_args(&[
            "for-each-ref",
            "--format=%(objectname)%00%(refname)%00%(symref)%00%(*objectname)%00%(*objecttype)%00%(objecttype)%00%(HEAD)%00%(upstream)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ]),
        "read history branches",
    )?;
    let mut refs = Vec::new();
    for line in output.stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split(|byte| *byte == 0).collect();
        if fields.len() != 8 {
            return Err(GitError::new("read history branches", "invalid ref record"));
        }
        let name = String::from_utf8_lossy(fields[1]);
        let (name, kind) = if let Some(name) = name.strip_prefix("refs/heads/") {
            (name, RefKind::Local)
        } else if let Some(name) = name.strip_prefix("refs/remotes/") {
            (name, RefKind::Remote)
        } else if let Some(name) = name.strip_prefix("refs/tags/") {
            (name, RefKind::Tag)
        } else {
            continue;
        };
        // origin/HEAD is an alias of a remote branch, not an extra branch tip.
        let commit = if !fields[2].is_empty() {
            None
        } else if kind == RefKind::Tag && !fields[3].is_empty() {
            (fields[4] == b"commit").then_some(fields[3])
        } else {
            (fields[5] == b"commit").then_some(fields[0])
        };
        let commit = commit.map(|hash| String::from_utf8_lossy(hash).into_owned());
        if let Some(hash) = &commit {
            validate_commit_hash(hash)?;
        }
        refs.push(HistoryRef {
            name: name.to_owned(),
            kind,
            commit,
            checked_out: fields[6] == b"*",
            upstream: (!fields[7].is_empty())
                .then(|| String::from_utf8_lossy(fields[7]).into_owned()),
        });
    }
    Ok(refs)
}

fn assemble_history_snapshot(
    root: &Path,
    previous: Option<&HistorySnapshot>,
    head: Option<String>,
    refs: Vec<HistoryRef>,
) -> GitResult<HistorySnapshot> {
    let mut snapshot = HistorySnapshot {
        upstream: refs
            .iter()
            .find(|reference| reference.checked_out)
            .and_then(|reference| reference.upstream.clone()),
        ..Default::default()
    };
    for reference in refs {
        let Some(hash) = reference.commit else {
            continue;
        };
        snapshot.refs.push(CommitRef {
            current: reference.kind == RefKind::Local
                && reference.checked_out
                && head.as_deref() == Some(&hash),
            hash,
            name: reference.name,
            kind: reference.kind,
        });
    }
    if let Some(head) = head {
        validate_commit_hash(&head)?;
        if !snapshot.refs.iter().any(|reference| reference.current) {
            snapshot.refs.push(CommitRef {
                hash: head,
                name: "HEAD".into(),
                kind: RefKind::Head,
                current: true,
            });
        }
    }
    snapshot.refs.sort_by(|left, right| {
        (!left.current, left.kind, &left.name).cmp(&(!right.current, right.kind, &right.name))
    });
    snapshot.tips = snapshot
        .refs
        .iter()
        .map(|reference| reference.hash.clone())
        .collect();
    snapshot.tips.sort();
    snapshot.tips.dedup();
    if let Some(previous) = previous
        && snapshot.tips == previous.tips
        && snapshot.refs == previous.refs
    {
        snapshot.local_only = previous.local_only.clone();
        return Ok(snapshot);
    }
    let remote_tips: Vec<_> = snapshot
        .refs
        .iter()
        .filter(|reference| reference.kind == RefKind::Remote)
        .map(|reference| reference.hash.as_str())
        .collect();
    if !remote_tips.is_empty() {
        let mut input = snapshot.tips.join("\n") + "\n";
        for hash in remote_tips {
            input.push('^');
            input.push_str(hash);
            input.push('\n');
        }
        let output = raw_git_with_input(
            root,
            &string_args(&["rev-list", "--stdin"]),
            "compare local and remote history",
            Some(input.into_bytes()),
        )?;
        if !output.status.success() {
            return Err(command_error("compare local and remote history", &output));
        }
        snapshot.local_only = std::sync::Arc::new(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::to_owned)
                .collect(),
        );
    }
    Ok(snapshot)
}

pub fn history_page(
    root: &Path,
    snapshot: &HistorySnapshot,
    offset: usize,
    limit: usize,
) -> GitResult<HistoryPage> {
    if limit == 0 || snapshot.tips.is_empty() {
        return Ok(HistoryPage::default());
    }
    for hash in &snapshot.tips {
        validate_commit_hash(hash)?;
    }
    let mut args = string_args(&[
        "log",
        "--topo-order",
        "--date=short",
        "--no-show-signature",
        "--format=%H%x00%P%x00%an%x00%aI%x00%at%x00%s%x00",
        "--stdin",
    ]);
    args.push(OsString::from(format!("--skip={offset}")));
    args.push(OsString::from(format!(
        "--max-count={}",
        limit.saturating_add(1)
    )));
    // stdin avoids Windows' command-line length limit for repositories with
    // thousands of branches. Only validated full object IDs enter this input.
    let input = snapshot.tips.join("\n") + "\n";
    let output = raw_git_with_input(root, &args, "read history page", Some(input.into_bytes()))?;
    if !output.status.success() {
        return Err(command_error("read history page", &output));
    }
    let mut commits = parse_log(&output.stdout)?;
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    for commit in &mut commits {
        commit.on_remote = snapshot.refs.iter().any(|r| r.kind == RefKind::Remote)
            && !snapshot.local_only.contains(&commit.hash);
        commit.refs = snapshot
            .refs
            .iter()
            .filter(|reference| reference.hash == commit.hash)
            .cloned()
            .collect();
    }
    Ok(HistoryPage { commits, has_more })
}

/// Switch a named local branch or create a tracking branch from an exact remote ref.
/// Git itself refuses worktree conflicts; never discard, force, stash or reset.
pub fn switch_branch(root: &Path, target: &str) -> GitResult<String> {
    let snapshot = history_snapshot(root)?;
    let reference = snapshot
        .refs
        .iter()
        .find(|reference| {
            let prefix = match reference.kind {
                RefKind::Local => "refs/heads/",
                RefKind::Remote => "refs/remotes/",
                _ => return false,
            };
            target == format!("{prefix}{}", reference.name)
        })
        .ok_or_else(|| {
            GitError::new(
                "switch branch",
                "branch no longer exists; refresh the branch list",
            )
        })?;
    let args = match reference.kind {
        RefKind::Local => string_args(&["switch", "--no-guess", "--", &reference.name]),
        RefKind::Remote => string_args(&["switch", "--track", "--", target]),
        _ => unreachable!(),
    };
    mutate_git(root, &args, "switch branch")?;
    Ok(current_branch(root)?.unwrap_or_default())
}

fn parse_log(bytes: &[u8]) -> GitResult<Vec<CommitSummary>> {
    let mut commits = Vec::new();
    let mut fields = bytes.split(|byte| *byte == 0);
    while let Some(hash) = fields.next() {
        let hash = String::from_utf8_lossy(hash).trim().to_owned();
        if hash.is_empty() {
            continue;
        }
        let (Some(parents), Some(author), Some(date), Some(timestamp), Some(subject)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(GitError::new(
                "parse recent history",
                "Git returned an unexpected log record",
            ));
        };
        validate_commit_hash(&hash)?;
        let parents: Vec<_> = String::from_utf8_lossy(parents)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        for parent in &parents {
            validate_commit_hash(parent)?;
        }
        commits.push(CommitSummary {
            hash,
            parents,
            refs: Vec::new(),
            author: String::from_utf8_lossy(author).into_owned(),
            date: String::from_utf8_lossy(date).into_owned(),
            timestamp: String::from_utf8_lossy(timestamp)
                .parse()
                .map_err(|_| GitError::new("parse recent history", "invalid author timestamp"))?,
            on_remote: false,
            subject: String::from_utf8_lossy(subject).into_owned(),
        });
    }
    Ok(commits)
}
