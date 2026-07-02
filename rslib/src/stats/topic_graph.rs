// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! MCAT cue-diagnostics knowledge graph.
//!
//! Given the AAMC topic graph (nodes = content categories, directed edges =
//! prerequisites) supplied by the caller, this measures per-node mastery from
//! the live cards (reusing the topic-mastery aggregation), then computes:
//!   * `mastered`       - node mean retrievability >= mastery_threshold,
//!   * `learning_ready` - every prerequisite is mastered (spaced/scaffolded
//!     learning: don't recommend a concept whose foundations are shaky),
//!   * `points_at_stake`- AAMC weight x forgetting risk (uncovered = full weight),
//!   * `recommended_path` - a prerequisite-respecting, points-at-stake-greedy
//!     traversal so the learner always has a well-founded next concept.
//!
//! Lives in the shared Rust engine so desktop and AnkiDroid render the same graph.

use std::collections::HashMap;
use std::collections::HashSet;

use anki_proto::stats::TopicGraphNodeStat;
use anki_proto::stats::TopicGraphRequest;
use anki_proto::stats::TopicGraphResponse;

use crate::prelude::*;

impl Collection {
    pub(crate) fn topic_graph(
        &mut self,
        input: TopicGraphRequest,
    ) -> Result<TopicGraphResponse> {
        let prefix = if input.topic_prefix.is_empty() {
            "mcat"
        } else {
            input.topic_prefix.as_str()
        };
        let mastery_threshold = if input.mastery_threshold > 0.0 {
            input.mastery_threshold
        } else {
            0.7
        };

        // Per-topic memory signal measured from the live collection.
        let mastery =
            self.topic_mastery_for_search(&input.search, prefix, input.mastered_threshold)?;
        let stats_by_id: HashMap<&str, &anki_proto::stats::TopicMasteryStats> = mastery
            .topics
            .iter()
            .map(|t| (t.topic_id.as_str(), t))
            .collect();

        // prerequisites: node -> list of prerequisite node ids.
        let mut prereqs: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &input.prereq_edges {
            prereqs.entry(e.to.as_str()).or_default().push(e.from.as_str());
        }

        // First pass: measured fields + mastered flag.
        struct NodeCalc {
            mastered: bool,
            points_at_stake: f32,
        }
        let mut calc: HashMap<String, NodeCalc> = HashMap::new();
        let mut out_nodes: Vec<TopicGraphNodeStat> = Vec::with_capacity(input.nodes.len());

        for node in &input.nodes {
            let st = stats_by_id.get(node.id.as_str());
            let card_count = st.map(|s| s.card_count).unwrap_or(0);
            let mastered_count = st.map(|s| s.mastered_count).unwrap_or(0);
            let mean_retr = st.map(|s| s.mean_retrievability).unwrap_or(0.0);
            let graded_reviews = st.map(|s| s.graded_reviews).unwrap_or(0);
            let covered = card_count > 0;
            let mastered = covered && mean_retr >= mastery_threshold;
            // uncovered topics carry their full weight (everything is at stake).
            let points_at_stake = if covered {
                node.weight * (1.0 - mean_retr)
            } else {
                node.weight
            };
            calc.insert(
                node.id.clone(),
                NodeCalc {
                    mastered,
                    points_at_stake,
                },
            );
            out_nodes.push(TopicGraphNodeStat {
                id: node.id.clone(),
                section: node.section.clone(),
                label: node.label.clone(),
                weight: node.weight,
                card_count,
                mastered_count,
                mean_retrievability: mean_retr,
                graded_reviews,
                covered,
                mastered,
                learning_ready: false, // filled below
                points_at_stake,
                unmet_prereqs: Vec::new(), // filled below
            });
        }

        let mastered_set: HashSet<&str> = calc
            .iter()
            .filter(|(_, c)| c.mastered)
            .map(|(id, _)| id.as_str())
            .collect();

        // Second pass: learning_ready + unmet prerequisites.
        for node in out_nodes.iter_mut() {
            if let Some(reqs) = prereqs.get(node.id.as_str()) {
                let unmet: Vec<String> = reqs
                    .iter()
                    .filter(|p| !mastered_set.contains(**p))
                    .map(|p| p.to_string())
                    .collect();
                node.learning_ready = unmet.is_empty();
                node.unmet_prereqs = unmet;
            } else {
                node.learning_ready = true;
            }
        }

        // Recommended traversal: greedy over unmastered nodes, respecting
        // prerequisites (a node becomes eligible once its prerequisites are
        // either already mastered or earlier in the path), picking the highest
        // points-at-stake eligible node each step.
        let pas: HashMap<&str, f32> = calc
            .iter()
            .map(|(id, c)| (id.as_str(), c.points_at_stake))
            .collect();
        let mut scheduled: HashSet<String> = mastered_set.iter().map(|s| s.to_string()).collect();
        let mut remaining: Vec<&str> = calc
            .iter()
            .filter(|(_, c)| !c.mastered)
            .map(|(id, _)| id.as_str())
            .collect();
        let mut path: Vec<String> = Vec::new();

        while !remaining.is_empty() {
            // eligible = all prereqs scheduled (mastered or already in path)
            let mut best: Option<(&str, f32)> = None;
            for &id in &remaining {
                let ready = prereqs
                    .get(id)
                    .map(|rs| rs.iter().all(|p| scheduled.contains(*p)))
                    .unwrap_or(true);
                if ready {
                    let score = *pas.get(id).unwrap_or(&0.0);
                    if best.map(|(_, bs)| score > bs).unwrap_or(true) {
                        best = Some((id, score));
                    }
                }
            }
            let chosen = match best {
                Some((id, _)) => id.to_string(),
                // No eligible node (would only happen with a prereq cycle); append
                // the remaining highest-stake node to guarantee progress.
                None => {
                    let mut fallback: Option<(&str, f32)> = None;
                    for &id in &remaining {
                        let score = *pas.get(id).unwrap_or(&0.0);
                        if fallback.map(|(_, bs)| score > bs).unwrap_or(true) {
                            fallback = Some((id, score));
                        }
                    }
                    fallback.map(|(id, _)| id.to_string()).unwrap()
                }
            };
            scheduled.insert(chosen.clone());
            remaining.retain(|&id| id != chosen);
            path.push(chosen);
        }

        // Deterministic node ordering for tests + stable layout.
        out_nodes.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(TopicGraphResponse {
            nodes: out_nodes,
            recommended_path: path,
        })
    }
}

#[cfg(test)]
mod test {
    use anki_proto::stats::TopicGraphEdge;
    use anki_proto::stats::TopicGraphNode;

    use super::*;
    use crate::card::CardType;
    use crate::card::FsrsMemoryState;

    fn add_card(col: &mut Collection, tag: &str, memory: Option<(f32, f32)>, backdate_days: i64) {
        let nt = col.get_notetype_by_name("Basic").unwrap().unwrap();
        let mut note = nt.new_note();
        note.tags = vec![tag.to_string()];
        col.add_note(&mut note, DeckId(1)).unwrap();
        let card_id = col
            .storage
            .all_cards_of_note(note.id)
            .unwrap()
            .pop()
            .unwrap()
            .id;
        let mut card = col.storage.get_card(card_id).unwrap().unwrap();
        card.reps = 5;
        if let Some((stability, difficulty)) = memory {
            card.memory_state = Some(FsrsMemoryState {
                stability,
                difficulty,
            });
            card.ctype = CardType::Review;
            card.last_review_time =
                Some(TimestampSecs(TimestampSecs::now().0 - backdate_days * 86400));
        }
        col.storage.update_card(&card).unwrap();
    }

    fn node(id: &str, section: &str, w: f32) -> TopicGraphNode {
        TopicGraphNode {
            id: id.to_string(),
            section: section.to_string(),
            label: id.to_string(),
            weight: w,
        }
    }

    fn edge(from: &str, to: &str) -> TopicGraphEdge {
        TopicGraphEdge {
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    fn req(nodes: Vec<TopicGraphNode>, edges: Vec<TopicGraphEdge>) -> TopicGraphRequest {
        TopicGraphRequest {
            search: String::new(),
            topic_prefix: "mcat".to_string(),
            mastered_threshold: 0.5,
            mastery_threshold: 0.7,
            nodes,
            prereq_edges: edges,
        }
    }

    #[test]
    fn learning_ready_gate_follows_prereq_mastery() -> Result<()> {
        let mut col = Collection::new();
        // A: freshly reviewed high-stability card -> mastered.
        add_card(&mut col, "mcat::bb::amino_acids_proteins", Some((1000.0, 5.0)), 0);
        // B (enzymes) depends on A; has a stale card -> not mastered, but ready.
        add_card(&mut col, "mcat::bb::enzymes", Some((5.0, 5.0)), 30);

        let nodes = vec![
            node("mcat::bb::amino_acids_proteins", "BB", 0.5),
            node("mcat::bb::enzymes", "BB", 0.5),
        ];
        let edges = vec![edge("mcat::bb::amino_acids_proteins", "mcat::bb::enzymes")];
        let resp = col.topic_graph(req(nodes, edges))?;

        let a = resp.nodes.iter().find(|n| n.id.ends_with("proteins")).unwrap();
        let b = resp.nodes.iter().find(|n| n.id.ends_with("enzymes")).unwrap();
        assert!(a.mastered, "A should be mastered");
        assert!(!b.mastered, "B should not be mastered yet");
        assert!(b.learning_ready, "B is ready because its prereq A is mastered");
        assert!(b.unmet_prereqs.is_empty());
        Ok(())
    }

    #[test]
    fn unmet_prereq_blocks_learning_ready() -> Result<()> {
        let mut col = Collection::new();
        // A not mastered (stale), B depends on A.
        add_card(&mut col, "mcat::bb::amino_acids_proteins", Some((5.0, 5.0)), 60);
        add_card(&mut col, "mcat::bb::enzymes", Some((5.0, 5.0)), 60);
        let nodes = vec![
            node("mcat::bb::amino_acids_proteins", "BB", 0.5),
            node("mcat::bb::enzymes", "BB", 0.5),
        ];
        let edges = vec![edge("mcat::bb::amino_acids_proteins", "mcat::bb::enzymes")];
        let resp = col.topic_graph(req(nodes, edges))?;
        let b = resp.nodes.iter().find(|n| n.id.ends_with("enzymes")).unwrap();
        assert!(!b.learning_ready, "B blocked because prereq A is not mastered");
        assert_eq!(b.unmet_prereqs, vec!["mcat::bb::amino_acids_proteins"]);
        Ok(())
    }

    #[test]
    fn recommended_path_respects_prereqs_and_stake() -> Result<()> {
        let mut col = Collection::new();
        // Nothing mastered: A -> B -> C chain, plus standalone D with high weight.
        add_card(&mut col, "mcat::bb::amino_acids_proteins", Some((5.0, 5.0)), 60);
        add_card(&mut col, "mcat::bb::enzymes", Some((5.0, 5.0)), 60);
        add_card(&mut col, "mcat::bb::metabolism", Some((5.0, 5.0)), 60);
        let nodes = vec![
            node("mcat::bb::amino_acids_proteins", "BB", 0.2),
            node("mcat::bb::enzymes", "BB", 0.2),
            node("mcat::bb::metabolism", "BB", 0.2),
            node("mcat::bb::organ_systems", "BB", 0.9), // high weight, no prereq -> first
        ];
        let edges = vec![
            edge("mcat::bb::amino_acids_proteins", "mcat::bb::enzymes"),
            edge("mcat::bb::enzymes", "mcat::bb::metabolism"),
        ];
        let resp = col.topic_graph(req(nodes, edges))?;
        let path = &resp.recommended_path;
        // organ_systems (no prereq, highest stake) should come first
        assert_eq!(path[0], "mcat::bb::organ_systems");
        // proteins must precede enzymes must precede metabolism
        let pos = |id: &str| path.iter().position(|x| x == id).unwrap();
        assert!(pos("mcat::bb::amino_acids_proteins") < pos("mcat::bb::enzymes"));
        assert!(pos("mcat::bb::enzymes") < pos("mcat::bb::metabolism"));
        Ok(())
    }

    #[test]
    fn uncovered_node_carries_full_weight() -> Result<()> {
        let mut col = Collection::new();
        // no cards for this topic at all
        let nodes = vec![node("mcat::cp::optics_sound", "CP", 0.3)];
        let resp = col.topic_graph(req(nodes, vec![]))?;
        let n = &resp.nodes[0];
        assert!(!n.covered);
        assert!(!n.mastered);
        assert!((n.points_at_stake - 0.3).abs() < 1e-6);
        Ok(())
    }
}
