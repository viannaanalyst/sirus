//! The lanes of a Git history, one row per commit, in the list's order (newest first). Pure, so
//! it is tested without a window; `panel_changes` draws the rows.

/// One commit's row: its lane, and the lines through the row's upper and lower halves as
/// (lane where they start, lane where they end).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Row {
    pub lane: usize,
    pub above: Vec<(usize, usize)>,
    pub below: Vec<(usize, usize)>,
}

/// The rows of `commits`, each given as its hash and its parents' hashes.
pub fn rows<'a>(commits: impl IntoIterator<Item = (&'a str, &'a [String])>) -> Vec<Row> {
    // Which commit each lane is waiting for, from left to right.
    let mut lanes: Vec<Option<&'a str>> = Vec::new();
    let mut out = Vec::new();
    for (hash, parents) in commits {
        let lane = lanes
            .iter()
            .position(|waiting| *waiting == Some(hash))
            .or_else(|| lanes.iter().position(Option::is_none))
            .unwrap_or_else(|| {
                lanes.push(None);
                lanes.len() - 1
            });
        // Every lane comes down straight, and those waiting for this commit bend into it.
        let above = lanes
            .iter()
            .enumerate()
            .filter_map(|(at, waiting)| {
                let waiting = (*waiting)?;
                Some((at, if waiting == hash { lane } else { at }))
            })
            .collect();
        for waiting in &mut lanes {
            if *waiting == Some(hash) {
                *waiting = None;
            }
        }
        let mut below: Vec<(usize, usize)> = lanes
            .iter()
            .enumerate()
            .filter_map(|(at, waiting)| waiting.map(|_| (at, at)))
            .collect();
        for (index, parent) in parents.iter().enumerate() {
            let parent = parent.as_str();
            // A farther lane waiting for the first parent folds into this one, so the main
            // line keeps to the left.
            if index == 0
                && lanes[lane].is_none()
                && let Some(far) = lanes.iter().position(|waiting| *waiting == Some(parent))
                && far > lane
            {
                lanes[far] = None;
                lanes[lane] = Some(parent);
                below.iter_mut().for_each(|edge| {
                    if edge.0 == far {
                        edge.1 = lane;
                    }
                });
                below.push((lane, lane));
                continue;
            }
            let target = lanes
                .iter()
                .position(|waiting| *waiting == Some(parent))
                .unwrap_or_else(|| {
                    // The first parent carries on down the commit's own lane.
                    let free = if index == 0 && lanes[lane].is_none() {
                        Some(lane)
                    } else {
                        lanes.iter().position(Option::is_none)
                    };
                    let free = free.unwrap_or_else(|| {
                        lanes.push(None);
                        lanes.len() - 1
                    });
                    lanes[free] = Some(parent);
                    free
                });
            below.push((lane, target));
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        out.push(Row { lane, above, below });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(list: &[(&str, &[&str])]) -> Vec<Row> {
        let owned: Vec<(String, Vec<String>)> = list
            .iter()
            .map(|(hash, parents)| {
                (
                    (*hash).to_owned(),
                    parents.iter().map(|p| (*p).to_owned()).collect(),
                )
            })
            .collect();
        rows(
            owned
                .iter()
                .map(|(hash, parents)| (hash.as_str(), parents.as_slice())),
        )
    }

    #[test]
    fn a_straight_history_stays_in_one_lane() {
        let graph = of(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]);
        assert!(graph.iter().all(|row| row.lane == 0));
        assert_eq!(graph[0].above, vec![]);
        assert_eq!(graph[0].below, vec![(0, 0)]);
        assert_eq!(graph[1].above, vec![(0, 0)]);
        assert_eq!(graph[2].below, vec![]);
    }

    #[test]
    fn a_merge_opens_a_lane_that_closes_at_the_fork() {
        let graph = of(&[("m", &["b", "x"]), ("x", &["a"]), ("b", &["a"]), ("a", &[])]);
        assert_eq!(graph[0].below, vec![(0, 0), (0, 1)]);
        // The side commit sits in the second lane and the first passes by.
        assert_eq!(graph[1].lane, 1);
        assert_eq!(graph[1].above, vec![(0, 0), (1, 1)]);
        assert_eq!(graph[1].below, vec![(0, 0), (1, 1)]);
        // At the fork the side lane bends into the main one, which stays on the left.
        assert_eq!(graph[2].lane, 0);
        assert_eq!(graph[2].below, vec![(1, 0), (0, 0)]);
        assert_eq!(graph[3].lane, 0);
        assert_eq!(graph[3].above, vec![(0, 0)]);
    }

    #[test]
    fn two_children_of_one_parent_share_the_parents_lane() {
        let graph = of(&[("t", &["p"]), ("u", &["p"]), ("p", &[])]);
        assert_eq!(graph[1].lane, 1);
        assert_eq!(graph[1].below, vec![(0, 0), (1, 0)]);
        assert_eq!(graph[2].above, vec![(0, 0)]);
    }
}
