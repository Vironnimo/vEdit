//! Incremental layout for a newest-first, topologically ordered commit history.
//!
//! The frontier contains the distinct commits that visible children still need
//! to reach. Rows retain their layout when another page is appended; only the
//! frontier continues into that page.
//!
//! The checked-out commit and its first-parent ancestry form the HEAD line.
//! The UI paints it in local and remote colors, like other editors' graphs,
//! while merged side branches keep their own track colors.

use crate::git::CommitSummary;
use std::collections::HashMap;

/// One segment between two lane positions in a half of a history row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GraphEdge {
    pub from_lane: usize,
    pub to_lane: usize,
    pub color: usize,
    /// The segment belongs to the HEAD line.
    pub head: bool,
    /// The child commit that starts this segment is on a fetched remote.
    pub remote: bool,
}

/// Incoming segments run from the row's top to its midpoint. Outgoing segments
/// run from its midpoint to its bottom, where they meet the next row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GraphRow {
    pub node_lane: usize,
    pub node_color: usize,
    /// The commit is HEAD or one of its first-parent ancestors.
    pub node_head: bool,
    pub lane_count: usize,
    pub incoming: Vec<GraphEdge>,
    pub outgoing: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Track {
    hash: String,
    color: usize,
    head: bool,
    remote: bool,
}

impl Track {
    fn edge(&self, from_lane: usize, to_lane: usize) -> GraphEdge {
        GraphEdge {
            from_lane,
            to_lane,
            color: self.color,
            head: self.head,
            remote: self.remote,
        }
    }
}

/// Layout for a history whose commits occur once, with children before parents.
/// Color indices identify tracks and can be mapped to the UI's own palette.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Graph {
    pub rows: Vec<GraphRow>,
    pub lane_count: usize,
    frontier: Vec<Track>,
    next_color: usize,
}

impl Graph {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn append(&mut self, commits: &[CommitSummary]) {
        self.rows.reserve(commits.len());
        for commit in commits {
            self.append_commit(commit);
        }
    }

    fn new_color(&mut self) -> usize {
        let color = self.next_color;
        self.next_color += 1;
        color
    }

    fn append_commit(&mut self, commit: &CommitSummary) {
        // Most histories spend long stretches on one lane. Keep that common
        // case free of frontier copies and lane lookups altogether.
        let current = commit.refs.iter().any(|reference| reference.current);
        if self.frontier.len() == 1
            && self.frontier[0].hash == commit.hash
            && commit.parents.len() <= 1
        {
            let track = &mut self.frontier[0];
            let incoming = track.edge(0, 0);
            let node_head = track.head || current;
            track.head = node_head;
            track.remote = commit.on_remote;
            let color = track.color;
            let outgoing = if let Some(parent) = commit.parents.first() {
                let edge = track.edge(0, 0);
                track.hash.clone_from(parent);
                vec![edge]
            } else {
                self.frontier.clear();
                Vec::new()
            };
            self.rows.push(GraphRow {
                node_lane: 0,
                node_color: color,
                node_head,
                lane_count: 1,
                incoming: vec![incoming],
                outgoing,
            });
            return;
        }
        let tracked_lane = self
            .frontier
            .iter()
            .position(|track| track.hash == commit.hash);
        let node_lane = tracked_lane.unwrap_or(self.frontier.len());
        let node_color = tracked_lane
            .map(|lane| self.frontier[lane].color)
            .unwrap_or_else(|| self.new_color());
        let node_head = current || tracked_lane.is_some_and(|lane| self.frontier[lane].head);
        let incoming = self
            .frontier
            .iter()
            .enumerate()
            .map(|(lane, track)| track.edge(lane, lane))
            .collect();

        let before = std::mem::take(&mut self.frontier);
        let mut after = before.clone();
        if let Some(lane) = tracked_lane {
            after.remove(lane);
        }

        // A new first parent carries the node's track forward. Additional
        // parents branch beside it. An already pending parent reuses its lane:
        // this joins only edges that really lead to the same commit.
        let mut insertion_lane = node_lane.min(after.len());
        let mut parents = Vec::with_capacity(commit.parents.len());
        for (parent_index, hash) in commit.parents.iter().enumerate() {
            if parents.contains(hash) {
                continue;
            }
            parents.push(hash.clone());
            if let Some(track) = after.iter_mut().find(|track| track.hash == *hash) {
                // The HEAD line continues through a parent that a side branch
                // reached first, so the shared lane below takes its colors.
                if parent_index == 0 && node_head {
                    track.head = true;
                    track.remote = commit.on_remote;
                }
                continue;
            }
            let color = if parent_index == 0 {
                node_color
            } else {
                self.new_color()
            };
            after.insert(
                insertion_lane,
                Track {
                    hash: hash.clone(),
                    color,
                    head: parent_index == 0 && node_head,
                    remote: commit.on_remote,
                },
            );
            insertion_lane += 1;
        }

        // Wide histories otherwise scan the whole frontier once for every
        // outgoing edge. Tiny frontiers stay on the allocation-free fast path.
        let lanes: Option<HashMap<_, _>> = (after.len() > 16).then(|| {
            after
                .iter()
                .enumerate()
                .map(|(lane, track)| (track.hash.as_str(), lane))
                .collect()
        });
        let find_lane = |hash: &str| match &lanes {
            Some(lanes) => lanes.get(hash).copied(),
            None => after.iter().position(|track| track.hash == hash),
        };
        let mut outgoing = Vec::with_capacity(before.len() + parents.len());
        for (lane, track) in before.iter().enumerate() {
            if Some(lane) == tracked_lane {
                continue;
            }
            let to_lane =
                find_lane(&track.hash).expect("an unrelated frontier track survives the row");
            outgoing.push(after[to_lane].edge(lane, to_lane));
        }
        for parent in parents {
            let to_lane = find_lane(&parent).expect("every parent has a frontier track");
            outgoing.push(GraphEdge {
                remote: commit.on_remote,
                ..after[to_lane].edge(node_lane, to_lane)
            });
        }

        let lane_count = before.len().max(after.len()).max(node_lane + 1);
        self.lane_count = self.lane_count.max(lane_count);
        self.rows.push(GraphRow {
            node_lane,
            node_color,
            node_head,
            lane_count,
            incoming,
            outgoing,
        });
        self.frontier = after;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> CommitSummary {
        CommitSummary {
            hash: hash.into(),
            author: "Author".into(),
            date: "2026-09-26".into(),
            timestamp: 0,
            on_remote: false,
            subject: hash.into(),
            parents: parents.iter().map(|hash| (*hash).into()).collect(),
            refs: Vec::new(),
        }
    }

    fn edge(from_lane: usize, to_lane: usize, color: usize) -> GraphEdge {
        GraphEdge {
            from_lane,
            to_lane,
            color,
            head: false,
            remote: false,
        }
    }

    fn assert_continuous(rows: &[GraphRow]) {
        for pair in rows.windows(2) {
            let mut bottoms: Vec<_> = pair[0]
                .outgoing
                .iter()
                .map(|edge| (edge.to_lane, edge.color, edge.head))
                .collect();
            bottoms.sort_unstable();
            bottoms.dedup();
            let mut tops: Vec<_> = pair[1]
                .incoming
                .iter()
                .map(|edge| (edge.from_lane, edge.color, edge.head))
                .collect();
            tops.sort_unstable();
            assert_eq!(bottoms, tops, "adjacent row boundaries must match");
        }
    }

    #[test]
    fn linear_history_has_one_continuous_track() {
        let mut graph = Graph::default();
        graph.append(&[
            commit("tip", &["middle"]),
            commit("middle", &["root"]),
            commit("root", &[]),
        ]);

        assert_eq!(graph.lane_count, 1);
        assert!(graph.rows[0].incoming.is_empty());
        assert_eq!(graph.rows[0].outgoing, vec![edge(0, 0, 0)]);
        assert_eq!(graph.rows[1].incoming, vec![edge(0, 0, 0)]);
        assert_eq!(graph.rows[1].outgoing, vec![edge(0, 0, 0)]);
        assert!(graph.rows[2].outgoing.is_empty());
        assert!(graph.frontier.is_empty());
        assert_continuous(&graph.rows);
    }

    #[test]
    fn merge_branches_split_and_join_at_the_shared_ancestor() {
        let mut graph = Graph::default();
        graph.append(&[
            commit("merge", &["main", "feature"]),
            commit("main", &["base"]),
            commit("feature", &["base"]),
            commit("base", &[]),
        ]);

        assert_eq!(graph.rows[0].outgoing, vec![edge(0, 0, 0), edge(0, 1, 1)]);
        assert_eq!(graph.rows[1].node_lane, 0);
        assert_eq!(graph.rows[2].node_lane, 1);
        assert_eq!(graph.rows[2].outgoing, vec![edge(0, 0, 0), edge(1, 0, 0)]);
        assert_eq!(graph.rows[3].incoming, vec![edge(0, 0, 0)]);
        assert!(graph.frontier.is_empty());
        assert_continuous(&graph.rows);
    }

    #[test]
    fn a_remote_branch_tip_starts_without_a_false_child_connection() {
        let mut graph = Graph::default();
        graph.append(&[
            commit("local", &["local-parent"]),
            commit("remote", &["remote-parent"]),
            commit("remote-parent", &[]),
            commit("local-parent", &[]),
        ]);

        assert_eq!(graph.rows[1].node_lane, 1);
        assert_eq!(graph.rows[1].incoming, vec![edge(0, 0, 0)]);
        assert_eq!(graph.rows[1].outgoing, vec![edge(0, 0, 0), edge(1, 1, 1)]);
        assert_eq!(graph.rows[2].outgoing, vec![edge(0, 0, 0)]);
        assert!(graph.frontier.is_empty());
        assert_continuous(&graph.rows);
    }

    #[test]
    fn collapsing_a_finished_lane_routes_other_pending_commits() {
        let mut graph = Graph::default();
        graph.append(&[
            commit("merge", &["root", "branch"]),
            commit("root", &[]),
            commit("branch", &["older"]),
            commit("older", &[]),
        ]);

        assert_eq!(graph.rows[1].node_lane, 0);
        assert_eq!(graph.rows[1].outgoing, vec![edge(1, 0, 1)]);
        assert_eq!(graph.rows[2].node_lane, 0);
        assert_eq!(graph.rows[2].node_color, 1);
        assert_continuous(&graph.rows);
    }

    #[test]
    fn several_merge_parents_reuse_existing_frontier_lanes() {
        let mut graph = Graph::default();
        graph.append(&[
            commit("tip", &["shared", "merge"]),
            commit("merge", &["shared", "side-a", "side-b", "shared"]),
        ]);

        assert_eq!(graph.rows[1].node_lane, 1);
        assert_eq!(graph.rows[1].outgoing.len(), 4);
        assert_eq!(graph.rows[1].outgoing[1], edge(1, 0, 0));
        assert_eq!(
            graph
                .frontier
                .iter()
                .map(|track| track.hash.as_str())
                .collect::<Vec<_>>(),
            ["shared", "side-a", "side-b"]
        );
        assert_continuous(&graph.rows);
    }

    #[test]
    fn appending_pages_matches_a_single_history_and_preserves_previous_rows() {
        let commits = [
            commit("merge", &["left", "right"]),
            commit("left", &["base"]),
            commit("other-tip", &["other-root"]),
            commit("right", &["base"]),
            commit("base", &[]),
            commit("other-root", &[]),
        ];
        let mut entire = Graph::default();
        entire.append(&commits);

        for split in 0..=commits.len() {
            let mut paged = Graph::default();
            paged.append(&commits[..split]);
            let previous_rows = paged.rows.clone();
            paged.append(&commits[split..]);
            assert_eq!(&paged.rows[..split], previous_rows);
            assert_eq!(paged, entire);
        }
        assert_continuous(&entire.rows);
    }

    #[test]
    fn wide_history_keeps_tracks_connected_across_page_boundaries() {
        let mut commits = Vec::new();
        for lane in 0..128 {
            commits.push(commit(&format!("tip-{lane}"), &[&format!("parent-{lane}")]));
        }
        // Consume the middle lanes first, repeatedly shifting the surviving
        // tracks, then join them at one shared base.
        for lane in (0..128).step_by(2).chain((1..128).step_by(2).rev()) {
            commits.push(commit(&format!("parent-{lane}"), &["base"]));
        }
        commits.push(commit("base", &[]));
        let mut entire = Graph::default();
        entire.append(&commits);
        let mut paged = Graph::default();
        for page in commits.chunks(60) {
            paged.append(page);
        }
        assert_eq!(paged, entire);
        assert_eq!(entire.lane_count, 128);
        assert!(entire.frontier.is_empty());
        assert_continuous(&entire.rows);
        for (lane, row) in entire.rows[..128].iter().enumerate() {
            assert_eq!(row.node_lane, lane);
            assert_eq!(row.node_color, lane);
        }
    }

    #[test]
    fn head_line_follows_first_parents_and_turns_remote_where_the_remote_starts() {
        let mut head = commit("head", &["merge"]);
        head.refs.push(crate::git::CommitRef {
            hash: "head".into(),
            name: "main".into(),
            kind: crate::git::RefKind::Local,
            current: true,
        });
        let mut merge = commit("merge", &["base", "side"]);
        merge.on_remote = true;
        let mut side = commit("side", &["base"]);
        side.on_remote = true;
        let mut base = commit("base", &[]);
        base.on_remote = true;
        let mut graph = Graph::default();
        // An unrelated branch tip reaches the shared base before the HEAD line.
        graph.append(&[commit("other", &["base"]), head, merge, side, base]);

        assert!(!graph.rows[0].node_head);
        assert!(graph.rows[1].node_head);
        assert!(graph.rows[2].node_head);
        assert!(
            !graph.rows[3].node_head,
            "a merged side branch keeps its own color"
        );
        assert!(
            graph.rows[4].node_head,
            "the HEAD line takes over a shared lane"
        );
        let head_edge = |row: &GraphRow, lane: usize| {
            *row.outgoing
                .iter()
                .find(|edge| edge.from_lane == lane && edge.head)
                .unwrap()
        };
        assert!(!head_edge(&graph.rows[1], graph.rows[1].node_lane).remote);
        assert!(head_edge(&graph.rows[2], graph.rows[2].node_lane).remote);
        assert!(
            graph.rows[2]
                .outgoing
                .iter()
                .any(|edge| !edge.head && edge.from_lane == graph.rows[2].node_lane)
        );
        assert_continuous(&graph.rows);
    }

    #[test]
    fn clear_starts_a_new_snapshot_with_fresh_tracks() {
        let mut graph = Graph::default();
        graph.append(&[commit("old-tip", &["unseen-parent"])]);
        graph.clear();
        graph.append(&[commit("new-root", &[])]);

        assert_eq!(graph.rows.len(), 1);
        assert_eq!(graph.rows[0].node_color, 0);
        assert!(graph.rows[0].incoming.is_empty());
        assert!(graph.frontier.is_empty());
    }
}
