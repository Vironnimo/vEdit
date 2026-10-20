//! Incremental layout for a newest-first, topologically ordered commit history.
//!
//! Each frontier track follows a parent until that commit's own row. Tracks
//! meeting the same ancestor stay separate until then, retaining their colors.
//! Rows retain their layout when another page is appended.
//!
//! The checked-out commit and its first-parent ancestry form the HEAD line.
//! The UI paints it in local and remote colors, like other editors' graphs,
//! while merged side branches keep their own track colors.

use crate::git::CommitSummary;

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
    frontier: Vec<Option<Track>>,
    next_color: usize,
    head: Option<String>,
}

impl Graph {
    pub fn reset(&mut self, head: Option<&str>) {
        *self = Self {
            head: head.map(str::to_owned),
            ..Self::default()
        };
    }

    pub fn append(&mut self, commits: &[CommitSummary]) {
        // Production supplies HEAD from the frozen refs, including when it is
        // on a later page. Also support complete histories supplied by callers.
        if self.rows.is_empty() && self.head.is_none() {
            self.head = commits
                .iter()
                .find(|commit| commit.refs.iter().any(|reference| reference.current))
                .map(|commit| commit.hash.clone());
        }
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
        let current = self.head.as_ref() == Some(&commit.hash);
        if self.frontier.len() == 1
            && self.frontier[0]
                .as_ref()
                .is_some_and(|track| track.hash == commit.hash)
            && commit.parents.len() <= 1
        {
            let track = self.frontier[0].as_mut().unwrap();
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
        let tracked_lane = self.frontier.iter().position(|track| {
            track
                .as_ref()
                .is_some_and(|track| track.hash == commit.hash)
        });
        let node_head = current
            || self
                .frontier
                .iter()
                .flatten()
                .any(|track| track.hash == commit.hash && track.head);
        let node_lane = if node_head {
            0
        } else {
            tracked_lane.unwrap_or_else(|| self.free_lane(usize::from(self.head.is_some())))
        };
        let node_color = tracked_lane
            .map(|lane| self.frontier[lane].as_ref().unwrap().color)
            .unwrap_or_else(|| self.new_color());
        let incoming = self
            .frontier
            .iter()
            .enumerate()
            .filter_map(|(lane, track)| {
                track.as_ref().map(|track| {
                    track.edge(
                        lane,
                        if track.hash == commit.hash {
                            node_lane
                        } else {
                            lane
                        },
                    )
                })
            })
            .collect();
        let before_len = self.frontier.len();
        for track in &mut self.frontier {
            if track
                .as_ref()
                .is_some_and(|track| track.hash == commit.hash)
            {
                *track = None;
            }
        }
        let mut outgoing: Vec<_> = self
            .frontier
            .iter()
            .enumerate()
            .filter_map(|(lane, track)| track.as_ref().map(|track| track.edge(lane, lane)))
            .collect();
        // Keep first parents on their existing lane. Additional parents use a
        // free lane on the right. Never merge pending edges early: even edges
        // to the same parent keep their identity until the parent's node.
        let mut parents = Vec::with_capacity(commit.parents.len());
        for (parent_index, hash) in commit.parents.iter().enumerate() {
            if parents.contains(hash) {
                continue;
            }
            parents.push(hash.clone());
            let (lane, color) = if parent_index == 0 {
                (node_lane, node_color)
            } else {
                (self.free_lane(node_lane + 1), self.new_color())
            };
            let track = Track {
                hash: hash.clone(),
                color,
                head: parent_index == 0 && node_head,
                remote: commit.on_remote,
            };
            outgoing.push(track.edge(node_lane, lane));
            if lane >= self.frontier.len() {
                self.frontier.resize(lane + 1, None);
            }
            self.frontier[lane] = Some(track);
        }
        while self.frontier.last().is_some_and(Option::is_none) {
            self.frontier.pop();
        }
        let lane_count = before_len.max(self.frontier.len()).max(node_lane + 1);
        self.lane_count = self.lane_count.max(lane_count);
        self.rows.push(GraphRow {
            node_lane,
            node_color,
            node_head,
            lane_count,
            incoming,
            outgoing,
        });
    }

    fn free_lane(&self, start: usize) -> usize {
        (start..self.frontier.len())
            .find(|&lane| self.frontier[lane].is_none())
            .unwrap_or(self.frontier.len().max(start))
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

    fn head(hash: &str, parents: &[&str]) -> CommitSummary {
        let mut commit = commit(hash, parents);
        commit.refs.push(crate::git::CommitRef {
            hash: hash.into(),
            name: "main".into(),
            kind: crate::git::RefKind::Local,
            current: true,
        });
        commit
    }

    #[test]
    fn current_branch_stays_left_when_another_tip_is_newer() {
        let mut graph = Graph::default();
        graph.append(&[
            commit("other", &["base"]),
            head("head", &["main"]),
            commit("main", &["base"]),
            commit("base", &[]),
        ]);
        for row in graph.rows.iter().filter(|row| row.node_head) {
            assert_eq!(row.node_lane, 0, "the current branch owns the left lane");
        }
        assert_continuous(&graph.rows);
    }

    #[test]
    fn side_branch_keeps_its_color_and_joins_only_at_the_common_commit() {
        let mut graph = Graph::default();
        graph.append(&[
            head("merge", &["main", "feature"]),
            commit("feature", &["base"]),
            commit("main", &["base"]),
            commit("base", &[]),
        ]);
        let side_color = graph.rows[1].node_color;
        assert!(
            graph.rows[2].outgoing.iter().any(|edge| {
                edge.from_lane == 1 && edge.to_lane == 1 && !edge.head && edge.color == side_color
            }),
            "the side branch must pass the main commit without joining it"
        );
        assert!(
            graph.rows[3].incoming.iter().any(|edge| {
                edge.from_lane == 1 && edge.to_lane == 0 && !edge.head && edge.color == side_color
            }),
            "the side branch joins at the common commit in its own color"
        );
        assert_continuous(&graph.rows);
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
        assert_eq!(graph.rows[2].outgoing, vec![edge(0, 0, 0), edge(1, 1, 1)]);
        assert_eq!(graph.rows[3].incoming, vec![edge(0, 0, 0), edge(1, 0, 1)]);
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
    fn finishing_a_lane_does_not_move_other_pending_commits() {
        let mut graph = Graph::default();
        graph.append(&[
            commit("merge", &["root", "branch"]),
            commit("root", &[]),
            commit("branch", &["older"]),
            commit("older", &[]),
        ]);

        assert_eq!(graph.rows[1].node_lane, 0);
        assert_eq!(graph.rows[1].outgoing, vec![edge(1, 1, 1)]);
        assert_eq!(graph.rows[2].node_lane, 1);
        assert_eq!(graph.rows[2].node_color, 1);
        assert_continuous(&graph.rows);
    }

    #[test]
    fn several_merge_parents_keep_separate_tracks_until_their_nodes() {
        let mut graph = Graph::default();
        graph.append(&[
            commit("tip", &["shared", "merge"]),
            commit("merge", &["shared", "side-a", "side-b", "shared"]),
        ]);

        assert_eq!(graph.rows[1].node_lane, 1);
        assert_eq!(graph.rows[1].outgoing.len(), 4);
        assert_eq!(graph.rows[1].outgoing[1], edge(1, 1, 1));
        assert_eq!(
            graph
                .frontier
                .iter()
                .flatten()
                .map(|track| track.hash.as_str())
                .collect::<Vec<_>>(),
            ["shared", "shared", "side-a", "side-b"]
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
    fn head_on_a_later_page_has_a_reserved_lane_without_moving_earlier_rows() {
        let commits = [
            commit("other", &["base"]),
            head("head", &["merge"]),
            commit("merge", &["main", "side"]),
            commit("side", &["base"]),
            commit("main", &["base"]),
            commit("base", &[]),
        ];
        let mut entire = Graph::default();
        entire.reset(Some("head"));
        entire.append(&commits);
        for split in 0..=commits.len() {
            let mut paged = Graph::default();
            paged.reset(Some("head"));
            paged.append(&commits[..split]);
            let previous = paged.rows.clone();
            paged.append(&commits[split..]);
            assert_eq!(&paged.rows[..split], previous);
            assert_eq!(paged, entire);
        }
        assert_eq!(entire.rows[0].node_lane, 1);
        assert!(
            entire
                .rows
                .iter()
                .filter(|row| row.node_head)
                .all(|row| row.node_lane == 0)
        );
        assert_continuous(&entire.rows);
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
        graph.reset(None);
        graph.append(&[commit("new-root", &[])]);

        assert_eq!(graph.rows.len(), 1);
        assert_eq!(graph.rows[0].node_color, 0);
        assert!(graph.rows[0].incoming.is_empty());
        assert!(graph.frontier.is_empty());
    }
}
