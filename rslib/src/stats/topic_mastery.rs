// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! MCAT readiness: per-topic mastery aggregation.
//!
//! Groups cards by the MCAT topic encoded in their note tags (e.g.
//! `mcat::bb::amino_acids_proteins`) and reports, per topic, the card-weighted
//! mean FSRS retrievability, the mastered-card count, and the graded-review
//! volume. This is the Memory layer (M_s) signal and the evidence the dashboard
//! attributes; coverage is computed by the caller against the AAMC outline.
//!
//! Lives in Rust (not Python) because it must aggregate over tens of thousands
//! of cards within the dashboard's latency budget; it ships to both the desktop
//! and the AnkiDroid build via the shared engine.

use std::collections::HashMap;

use anki_proto::stats::TopicMasteryResponse;
use anki_proto::stats::TopicMasteryStats;
use fsrs::FSRS;
use fsrs::FSRS5_DEFAULT_DECAY;

use crate::prelude::*;
use crate::scheduler::timing::SchedTimingToday;
use crate::search::SortMode;

#[derive(Default)]
struct TopicAccum {
    card_count: u32,
    mastered_count: u32,
    retrievability_sum: f32,
    cards_with_state: u32,
    graded_reviews: u32,
}

impl Collection {
    /// Aggregate per-topic mastery over the cards matched by `search`.
    pub(crate) fn topic_mastery_for_search(
        &mut self,
        search: &str,
        topic_prefix: &str,
        mastered_threshold: f32,
    ) -> Result<TopicMasteryResponse> {
        let guard = self.search_cards_into_table(search, SortMode::NoOrder)?;
        let cards = guard.col.storage.all_searched_cards()?;
        let timing = guard.col.timing_today()?;
        let timing = SchedTimingToday {
            days_elapsed: timing.days_elapsed,
            now: TimestampSecs::now(),
            next_day_at: timing.next_day_at,
        };
        let fsrs = FSRS::new(None).unwrap();

        // Cache note tags so we touch each note at most once.
        let mut tags_cache: HashMap<NoteId, Vec<String>> = HashMap::new();
        let mut topics: HashMap<String, TopicAccum> = HashMap::new();

        for card in &cards {
            let tags = match tags_cache.get(&card.note_id) {
                Some(t) => t,
                None => {
                    let note_tags = guard
                        .col
                        .storage
                        .get_note(card.note_id)?
                        .map(|n| n.tags)
                        .unwrap_or_default();
                    tags_cache.entry(card.note_id).or_insert(note_tags)
                }
            };

            let retr = card.memory_state.map(|state| {
                let elapsed = card.seconds_since_last_review(&timing).unwrap_or_default();
                fsrs.current_retrievability_seconds(
                    state.into(),
                    elapsed,
                    card.decay.unwrap_or(FSRS5_DEFAULT_DECAY),
                )
            });

            for topic in matching_topics(tags, topic_prefix) {
                let acc = topics.entry(topic).or_default();
                acc.card_count += 1;
                acc.graded_reviews += card.reps;
                if let Some(r) = retr {
                    acc.retrievability_sum += r;
                    acc.cards_with_state += 1;
                    if r > mastered_threshold {
                        acc.mastered_count += 1;
                    }
                }
            }
        }

        let mut out: Vec<TopicMasteryStats> = topics
            .into_iter()
            .map(|(topic_id, acc)| TopicMasteryStats {
                topic_id,
                card_count: acc.card_count,
                mastered_count: acc.mastered_count,
                mean_retrievability: if acc.cards_with_state > 0 {
                    acc.retrievability_sum / acc.cards_with_state as f32
                } else {
                    0.0
                },
                graded_reviews: acc.graded_reviews,
            })
            .collect();
        // Stable, deterministic ordering for tests and the dashboard.
        out.sort_by(|a, b| a.topic_id.cmp(&b.topic_id));
        Ok(TopicMasteryResponse { topics: out })
    }
}

impl Collection {
    /// Order cards by points-at-stake = topic_weight * (1 - retrievability), so the
    /// highest-value cards (heavily weighted + most likely forgotten) come first.
    /// Uncovered/unseen cards are excluded (nothing to review yet).
    pub(crate) fn points_at_stake_order(
        &mut self,
        search: &str,
        topic_weights: &std::collections::HashMap<String, f32>,
        mastered_threshold: f32,
    ) -> Result<anki_proto::stats::PointsAtStakeResponse> {
        let _ = mastered_threshold; // reserved for future filtering
        let guard = self.search_cards_into_table(search, SortMode::NoOrder)?;
        let cards = guard.col.storage.all_searched_cards()?;
        let timing = guard.col.timing_today()?;
        let timing = SchedTimingToday {
            days_elapsed: timing.days_elapsed,
            now: TimestampSecs::now(),
            next_day_at: timing.next_day_at,
        };
        let fsrs = FSRS::new(None).unwrap();
        let mut tags_cache: HashMap<NoteId, Vec<String>> = HashMap::new();
        let mut out: Vec<anki_proto::stats::PointsAtStakeCard> = Vec::new();

        for card in &cards {
            let state = match card.memory_state {
                Some(s) => s,
                None => continue,
            };
            let tags = match tags_cache.get(&card.note_id) {
                Some(t) => t,
                None => {
                    let nt = guard
                        .col
                        .storage
                        .get_note(card.note_id)?
                        .map(|n| n.tags)
                        .unwrap_or_default();
                    tags_cache.entry(card.note_id).or_insert(nt)
                }
            };
            // pick the highest-weighted matching topic tag for this card
            let mut best: Option<(&String, f32)> = None;
            for tag in tags.iter() {
                if let Some(w) = topic_weights.get(tag) {
                    if best.map(|(_, bw)| *w > bw).unwrap_or(true) {
                        best = Some((tag, *w));
                    }
                }
            }
            let (topic_id, weight) = match best {
                Some((t, w)) => (t.clone(), w),
                None => continue,
            };
            let elapsed = card.seconds_since_last_review(&timing).unwrap_or_default();
            let r = fsrs.current_retrievability_seconds(
                state.into(),
                elapsed,
                card.decay.unwrap_or(FSRS5_DEFAULT_DECAY),
            );
            out.push(anki_proto::stats::PointsAtStakeCard {
                card_id: card.id.0,
                topic_id,
                points_at_stake: weight * (1.0 - r),
                retrievability: r,
            });
        }
        out.sort_by(|a, b| {
            b.points_at_stake
                .partial_cmp(&a.points_at_stake)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(anki_proto::stats::PointsAtStakeResponse { cards: out })
    }
}

/// Tags under the namespace are MCAT topics. `mcat` matches `mcat::bb::x`.
/// A bare `mcat` tag (no subtopic) is ignored - it is not a topic.
fn matching_topics(tags: &[String], prefix: &str) -> Vec<String> {
    let needle = format!("{prefix}::");
    tags.iter()
        .filter(|t| t.starts_with(&needle) && t.len() > needle.len())
        .cloned()
        .collect()
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::card::CardType;
    use crate::card::FsrsMemoryState;

    fn add_tagged_card(
        col: &mut Collection,
        tags: &[&str],
        memory: Option<(f32, f32)>,
        reps: u32,
    ) -> CardId {
        let nt = col.get_notetype_by_name("Basic").unwrap().unwrap();
        let mut note = nt.new_note();
        note.tags = tags.iter().map(|s| s.to_string()).collect();
        col.add_note(&mut note, DeckId(1)).unwrap();
        // fetch the card that was just created for this note
        let card_id = col.storage.all_cards_of_note(note.id).unwrap().pop().unwrap().id;
        let mut card = col.storage.get_card(card_id).unwrap().unwrap();
        card.reps = reps;
        if let Some((stability, difficulty)) = memory {
            card.memory_state = Some(FsrsMemoryState { stability, difficulty });
            card.ctype = CardType::Review;
            // make the review "now" so retrievability is high and deterministic
            card.last_review_time = Some(TimestampSecs::now());
        }
        col.storage.update_card(&card).unwrap();
        card_id
    }

    #[test]
    fn groups_cards_by_topic_tag() -> Result<()> {
        let mut col = Collection::new();
        add_tagged_card(&mut col, &["mcat::bb::enzymes"], Some((100.0, 5.0)), 4);
        add_tagged_card(&mut col, &["mcat::bb::enzymes"], Some((100.0, 5.0)), 2);
        add_tagged_card(&mut col, &["mcat::cp::kinetics"], Some((100.0, 5.0)), 7);

        let resp = col.topic_mastery_for_search("", "mcat", 0.5)?;
        assert_eq!(resp.topics.len(), 2);
        let enz = resp.topics.iter().find(|t| t.topic_id == "mcat::bb::enzymes").unwrap();
        assert_eq!(enz.card_count, 2);
        assert_eq!(enz.graded_reviews, 6); // 4 + 2
        let kin = resp.topics.iter().find(|t| t.topic_id == "mcat::cp::kinetics").unwrap();
        assert_eq!(kin.card_count, 1);
        assert_eq!(kin.graded_reviews, 7);
        Ok(())
    }

    #[test]
    fn mastered_threshold_counts_high_retrievability() -> Result<()> {
        let mut col = Collection::new();
        // high stability + reviewed now -> retrievability ~1.0 -> mastered
        add_tagged_card(&mut col, &["mcat::bb::enzymes"], Some((1000.0, 5.0)), 3);
        let resp = col.topic_mastery_for_search("", "mcat", 0.9)?;
        let enz = &resp.topics[0];
        assert_eq!(enz.mastered_count, 1);
        assert!(enz.mean_retrievability > 0.9);
        Ok(())
    }

    #[test]
    fn untagged_and_new_cards_are_excluded_or_zeroed() -> Result<()> {
        let mut col = Collection::new();
        // untagged card -> not a topic at all
        add_tagged_card(&mut col, &[], Some((100.0, 5.0)), 1);
        // tagged but new (no memory state) -> counted, but contributes 0 retrievability
        add_tagged_card(&mut col, &["mcat::ps::learning_memory"], None, 0);

        let resp = col.topic_mastery_for_search("", "mcat", 0.5)?;
        assert_eq!(resp.topics.len(), 1);
        let lm = &resp.topics[0];
        assert_eq!(lm.topic_id, "mcat::ps::learning_memory");
        assert_eq!(lm.card_count, 1);
        assert_eq!(lm.mastered_count, 0);
        assert_eq!(lm.mean_retrievability, 0.0);
        Ok(())
    }

    #[test]
    fn bare_prefix_tag_is_not_a_topic() {
        let tags = vec!["mcat".to_string(), "mcat::bb::enzymes".to_string()];
        let topics = matching_topics(&tags, "mcat");
        assert_eq!(topics, vec!["mcat::bb::enzymes".to_string()]);
    }

    #[test]
    fn points_at_stake_orders_high_weight_first() -> Result<()> {
        let mut col = Collection::new();
        let a = add_tagged_card(&mut col, &["mcat::bb::enzymes"], Some((10.0, 5.0)), 3);
        let b = add_tagged_card(&mut col, &["mcat::cp::optics_sound"], Some((10.0, 5.0)), 3);
        // backdate both reviews 30 days so retrievability < 1 (equal for both)
        for cid in [a, b] {
            let mut card = col.storage.get_card(cid)?.unwrap();
            card.last_review_time = Some(TimestampSecs(TimestampSecs::now().0 - 30 * 86400));
            col.storage.update_card(&card)?;
        }
        let mut weights = std::collections::HashMap::new();
        weights.insert("mcat::bb::enzymes".to_string(), 0.5f32);
        weights.insert("mcat::cp::optics_sound".to_string(), 0.2f32);
        let resp = col.points_at_stake_order("", &weights, 0.7)?;
        assert_eq!(resp.cards.len(), 2);
        // higher topic weight (same forgetting risk) -> higher stake -> first
        assert_eq!(resp.cards[0].card_id, a.0);
        assert!(resp.cards[0].points_at_stake > resp.cards[1].points_at_stake);
        assert!(resp.cards[0].retrievability < 1.0);
        Ok(())
    }
}
