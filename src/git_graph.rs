//! Incremental layout for a newest-first, topologically ordered commit history.
//!
//! The frontier contains the distinct commits that visible children still need
//! to reach. Rows retain their layout when another page is appended; only the
//! frontier continues into that page.

use crate::git::CommitSummary;

/// One segment between two lane positions in a half of a history row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GraphEdge {
    pub from_lane: usize,
    pub to_lane: usize,
    pub color: usize,
}

/// Incoming segments run from the row's top to its midpoint. Outgoing segments
/// run from its midpoint to its bottom, where they meet the next row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GraphRow {
    pub node_lane: usize,
    pub node_color: usize,
    pub lane_count: usize,
    pub incoming: Vec<GraphEdge>,
    pub outgoing: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Track {
    hash: String,
    color: usize,
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
        let tracked_lane = self
            .frontier
            .iter()
            .position(|track| track.hash == commit.hash);
        let node_lane = tracked_lane.unwrap_or(self.frontier.len());
        let node_color = tracked_lane
            .map(|lane| self.frontier[lane].color)
            .unwrap_or_else(|| self.new_color());
        let incoming = self
            .frontier
            .iter()
            .enumerate()
            .map(|(lane, track)| GraphEdge {
                from_lane: lane,
                to_lane: lane,
                color: track.color,
            })
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
            if after.iter().any(|track| track.hash == *hash) {
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
                },
            );
            insertion_lane += 1;
        }

        let mut outgoing = Vec::with_capacity(before.len() + parents.len());
        for (lane, track) in before.iter().enumerate() {
            if Some(lane) == tracked_lane {
                continue;
            }
            let to_lane = after
                .iter()
                .position(|pending| pending.hash == track.hash)
                .expect("an unrelated frontier track survives the row");
            outgoing.push(GraphEdge {
                from_lane: lane,
                to_lane,
                color: track.color,
            });
        }
        for parent in parents {
            let to_lane = after
                .iter()
                .position(|track| track.hash == parent)
                .expect("every parent has a frontier track");
            outgoing.push(GraphEdge {
                from_lane: node_lane,
                to_lane,
                color: after[to_lane].color,
            });
        }

        let lane_count = before.len().max(after.len()).max(node_lane + 1);
        self.lane_count = self.lane_count.max(lane_count);
        self.rows.push(GraphRow {
            node_lane,
            node_color,
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
        }
    }

    fn assert_continuous(rows: &[GraphRow]) {
        for pair in rows.windows(2) {
            let mut bottoms: Vec<_> = pair[0]
                .outgoing
                .iter()
                .map(|edge| (edge.to_lane, edge.color))
                .collect();
            bottoms.sort_unstable();
            bottoms.dedup();
            let mut tops: Vec<_> = pair[1]
                .incoming
                .iter()
                .map(|edge| (edge.from_lane, edge.color))
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
