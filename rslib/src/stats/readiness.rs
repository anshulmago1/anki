// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! MCAT readiness inference: the three separated scores (Memory, Performance,
//! Readiness), each with a range and its evidence, with the give-up rule
//! enforced. This runs in the engine so desktop and AnkiDroid show identical
//! numbers from one implementation; model *training* stays offline and is passed
//! in via `ReadinessParams` / per-section performance inputs.
//!
//! Cross-cutting rule (the honesty rule): no score is returned without its
//! evidence. If a section lacks data, its readiness is `abstained` with the
//! missing requirements listed, and the total abstains too.

use anki_proto::stats::EvidencedScore;
use anki_proto::stats::NextAction;
use anki_proto::stats::ReadinessRequest;
use anki_proto::stats::ReadinessResponse;
use anki_proto::stats::ReadinessSection;
use anki_proto::stats::ReadinessSectionInput;

use crate::prelude::*;

const SECTION_MIN: f32 = 118.0;
const SECTION_SPAN: f32 = 14.0; // 118..132

// Learning-science multiplier citations (Brainlift / Speedrun), surfaced as the
// method/driver text so a multiplier never appears without its evidence.
const MEMORY_METHOD: &str = "FSRS-6 power-law retrievability (benchmark log loss ~0.345)";
const PERF_METHOD: &str = "3PL IRT ability on held-out exam-style questions";
const READINESS_METHOD: &str = "logit map; community regression AAMC FL1+FL2 vs real MCAT r~0.86";

impl Collection {
    pub(crate) fn compute_readiness(
        &mut self,
        req: ReadinessRequest,
    ) -> Result<ReadinessResponse> {
        let params = req.params.unwrap_or(default_params());
        let mut sections = Vec::with_capacity(req.sections.len());
        let mut actions: Vec<NextAction> = Vec::new();
        let mut any_abstain = false;
        let mut total_value = 0.0f32;
        let mut total_var = 0.0f32;

        for sec in &req.sections {
            let (section, mut section_actions) =
                self.readiness_for_section(&req.search, req.mastered_threshold, sec, &params)?;
            actions.append(&mut section_actions);
            if let Some(r) = &section.readiness {
                if r.abstained {
                    any_abstain = true;
                } else {
                    total_value += r.value;
                    let hw = (r.range_hi - r.range_lo) / 2.0;
                    total_var += hw * hw;
                }
            }
            sections.push(section);
        }

        // Global "what to do next": highest points-at-stake first. Always present,
        // and most useful precisely when readiness is withheld.
        actions.sort_by(|a, b| {
            b.points_at_stake
                .partial_cmp(&a.points_at_stake)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        actions.truncate(6);

        let hw = total_var.sqrt();
        let total = EvidencedScore {
            value: if any_abstain { 0.0 } else { total_value },
            range_lo: (total_value - hw).max(472.0),
            range_hi: (total_value + hw).min(528.0),
            confidence: overall_confidence(&sections),
            graded_reviews: sections
                .iter()
                .filter_map(|s| s.readiness.as_ref().map(|r| r.graded_reviews))
                .sum(),
            coverage_pct: 0.0,
            method: READINESS_METHOD.to_string(),
            abstained: any_abstain,
            missing: if any_abstain {
                sections
                    .iter()
                    .filter(|s| s.readiness.as_ref().map(|r| r.abstained).unwrap_or(false))
                    .map(|s| s.section.clone())
                    .collect()
            } else {
                vec![]
            },
            drivers: vec![],
        };

        Ok(ReadinessResponse {
            sections,
            total: Some(total),
            next_actions: actions,
        })
    }

    fn readiness_for_section(
        &mut self,
        search: &str,
        mastered_threshold: f32,
        sec: &ReadinessSectionInput,
        params: &anki_proto::stats::ReadinessParams,
    ) -> Result<(ReadinessSection, Vec<NextAction>)> {
        // Memory is measured live from cards via the mastery query.
        let mastery = self.topic_mastery_for_search(search, &sec.topic_prefix, mastered_threshold)?;
        let by_topic: std::collections::HashMap<&str, &anki_proto::stats::TopicMasteryStats> =
            mastery.topics.iter().map(|t| (t.topic_id.as_str(), t)).collect();

        let mut m_s = 0.0f32;
        let mut graded: u32 = 0;
        let mut covered = 0usize;
        let total_topics = sec.topic_weights.len().max(1);
        let mut best_topic = String::new();
        let mut best_stake = -1.0f32;
        let mut actions: Vec<NextAction> = Vec::new();

        for (topic_id, weight) in &sec.topic_weights {
            let (mean_r, has_cards) = by_topic
                .get(topic_id.as_str())
                .map(|t| (t.mean_retrievability, t.card_count > 0))
                .unwrap_or((0.0, false));
            m_s += weight * mean_r;
            if has_cards {
                covered += 1;
                graded += by_topic.get(topic_id.as_str()).map(|t| t.graded_reviews).unwrap_or(0);
            }
            // points-at-stake: uncovered topic is maximum stake (whole weight)
            let stake = if has_cards { weight * (1.0 - mean_r) } else { *weight };
            if stake > best_stake {
                best_stake = stake;
                best_topic = topic_id.clone();
            }
            // candidate next action for this topic
            if !has_cards {
                actions.push(NextAction {
                    section: sec.section.clone(),
                    topic_id: topic_id.clone(),
                    action_type: "cover".to_string(),
                    points_at_stake: *weight,
                    reason: format!(
                        "Untested: no cards for this topic (exam weight {:.0}% of {}). Cover blind \
                         spots first so readiness isn't inflated by what you skipped.",
                        weight * 100.0,
                        sec.section
                    ),
                });
            } else if mean_r < 0.9 {
                actions.push(NextAction {
                    section: sec.section.clone(),
                    topic_id: topic_id.clone(),
                    action_type: "review".to_string(),
                    points_at_stake: stake,
                    reason: format!(
                        "~{:.0}% likely lapsed (retrievability {:.2}). Reviewing at the forgetting \
                         edge gives the biggest durable-memory gain (spacing effect).",
                        (1.0 - mean_r) * 100.0,
                        mean_r
                    ),
                });
            }
        }

        let coverage = covered as f32 / total_topics as f32;
        let conf = confidence(graded, coverage, sec.theta_se, params);

        let memory = EvidencedScore {
            value: m_s,
            range_lo: (m_s - band_hw(0.05, &conf)).max(0.0),
            range_hi: (m_s + band_hw(0.05, &conf)).min(1.0),
            confidence: conf.clone(),
            graded_reviews: graded,
            coverage_pct: coverage * 100.0,
            method: MEMORY_METHOD.to_string(),
            abstained: false,
            missing: vec![],
            drivers: vec![format!("{} mean retrievability {:.2} over {} reviews", sec.section, m_s, graded)],
        };

        let p_s = sec.performance;
        let performance = EvidencedScore {
            value: p_s,
            range_lo: (p_s - band_hw((sec.theta_se * 0.1).max(0.03), &conf)).max(0.0),
            range_hi: (p_s + band_hw((sec.theta_se * 0.1).max(0.03), &conf)).min(1.0),
            confidence: conf.clone(),
            graded_reviews: graded,
            coverage_pct: coverage * 100.0,
            method: PERF_METHOD.to_string(),
            abstained: false,
            missing: vec![],
            drivers: vec![format!("IRT theta SE {:.2}", sec.theta_se)],
        };

        // give-up rule
        let mut missing = vec![];
        if graded < params.min_graded_reviews {
            missing.push(format!("{}/{} graded reviews", graded, params.min_graded_reviews));
        }
        if coverage < params.min_coverage {
            missing.push(format!(
                "coverage {:.0}% < {:.0}%",
                coverage * 100.0,
                params.min_coverage * 100.0
            ));
        }
        if sec.theta_se > params.max_irt_se {
            missing.push(format!("IRT SE {:.2} > {:.2}", sec.theta_se, params.max_irt_se));
        }

        let mfac = mult_or_one(sec.alpha_space) * mult_or_one(sec.alpha_inter) * mult_or_one(sec.alpha_test);
        let lin = params.b0 + params.b_m * m_s * mfac + params.b_p * p_s + params.b_c * coverage;
        let e_s = SECTION_MIN + SECTION_SPAN * sigmoid(lin);
        let hw = 1.0 + 6.0 * sec.theta_se.min(0.5) + 4.0 * (params.min_coverage - coverage).max(0.0);

        let mut drivers = vec![
            format!("M={:.2}", m_s),
            format!("P={:.2}", p_s),
            format!("coverage={:.0}%", coverage * 100.0),
        ];
        if (mfac - 1.0).abs() > 1e-6 {
            drivers.push(format!("learning-science multiplier x{:.2}", mfac));
        }

        let readiness = EvidencedScore {
            value: e_s,
            range_lo: (e_s - hw).max(SECTION_MIN),
            range_hi: (e_s + hw).min(SECTION_MIN + SECTION_SPAN),
            confidence: conf,
            graded_reviews: graded,
            coverage_pct: coverage * 100.0,
            method: READINESS_METHOD.to_string(),
            abstained: !missing.is_empty(),
            missing,
            drivers,
        };

        // Section-level action: if there's content to test but no transfer
        // evidence, recommend a question check (recognition != transfer).
        if covered > 0 && sec.theta_se > params.max_irt_se {
            actions.push(NextAction {
                section: sec.section.clone(),
                topic_id: String::new(),
                action_type: "practice_check".to_string(),
                points_at_stake: 0.45,
                reason: format!(
                    "No transfer evidence for {} yet. Take a question check so Performance can be \
                     estimated - recalling a card is not the same as answering a passage.",
                    sec.section
                ),
            });
        }

        let section = ReadinessSection {
            section: sec.section.clone(),
            memory: Some(memory),
            performance: Some(performance),
            readiness: Some(readiness),
            next_best_topic: best_topic,
            next_best_points_at_stake: best_stake.max(0.0),
        };
        Ok((section, actions))
    }
}

fn default_params() -> anki_proto::stats::ReadinessParams {
    anki_proto::stats::ReadinessParams {
        b0: -1.0,
        b_m: 1.3,
        b_p: 2.2,
        b_c: 0.5,
        min_graded_reviews: 200,
        min_coverage: 0.5,
        max_irt_se: 0.5,
    }
}

fn mult_or_one(m: f32) -> f32 {
    if m <= 0.0 {
        1.0
    } else {
        m
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn confidence(reviews: u32, coverage: f32, se: f32, params: &anki_proto::stats::ReadinessParams) -> String {
    if reviews >= params.min_graded_reviews && coverage >= 0.6 && se <= 0.4 {
        "high".to_string()
    } else if reviews >= params.min_graded_reviews / 2 && coverage >= params.min_coverage {
        "moderate".to_string()
    } else {
        "low".to_string()
    }
}

fn band_hw(base: f32, conf: &str) -> f32 {
    let mult = match conf {
        "high" => 1.0,
        "moderate" => 1.6,
        _ => 2.4,
    };
    base * mult
}

fn overall_confidence(sections: &[ReadinessSection]) -> String {
    let mut worst = 2; // 0 high, 1 moderate, 2 low
    for s in sections {
        if let Some(r) = &s.readiness {
            let rank = match r.confidence.as_str() {
                "high" => 0,
                "moderate" => 1,
                _ => 2,
            };
            worst = worst.min(rank); // best across sections for display
            let _ = rank;
        }
    }
    match worst {
        0 => "high",
        1 => "moderate",
        _ => "low",
    }
    .to_string()
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::card::CardType;
    use crate::card::FsrsMemoryState;
    use std::collections::HashMap;

    fn add(col: &mut Collection, tag: &str, mem: Option<(f32, f32)>, reps: u32) {
        let nt = col.get_notetype_by_name("Basic").unwrap().unwrap();
        let mut note = nt.new_note();
        note.tags = vec![tag.to_string()];
        col.add_note(&mut note, DeckId(1)).unwrap();
        let cid = col.storage.all_cards_of_note(note.id).unwrap().pop().unwrap().id;
        let mut card = col.storage.get_card(cid).unwrap().unwrap();
        card.reps = reps;
        if let Some((s, d)) = mem {
            card.memory_state = Some(FsrsMemoryState { stability: s, difficulty: d });
            card.ctype = CardType::Review;
            card.last_review_time = Some(TimestampSecs::now());
        }
        col.storage.update_card(&card).unwrap();
    }

    fn section(prefix: &str, weights: &[(&str, f32)], perf: f32) -> ReadinessSectionInput {
        ReadinessSectionInput {
            section: "BB".to_string(),
            topic_prefix: prefix.to_string(),
            topic_weights: weights.iter().map(|(k, v)| (k.to_string(), *v)).collect::<HashMap<_, _>>(),
            performance: perf,
            theta_se: 0.3,
            alpha_space: 1.0,
            alpha_inter: 1.0,
            alpha_test: 1.0,
        }
    }

    #[test]
    fn abstains_when_coverage_below_threshold() -> Result<()> {
        let mut col = Collection::new();
        // only one of two topics covered -> coverage 50%, but reviews far below 200
        add(&mut col, "mcat::bb::enzymes", Some((1000.0, 5.0)), 3);
        let sec = section("mcat::bb", &[("mcat::bb::enzymes", 0.5), ("mcat::bb::nucleic_acids", 0.5)], 0.6);
        let resp = col.compute_readiness(ReadinessRequest {
            search: String::new(),
            mastered_threshold: 0.7,
            sections: vec![sec],
            params: None,
        })?;
        let rd = resp.sections[0].readiness.as_ref().unwrap();
        assert!(rd.abstained);
        assert!(rd.missing.iter().any(|m| m.contains("graded reviews")));
        assert!(resp.total.as_ref().unwrap().abstained);
        Ok(())
    }

    #[test]
    fn memory_reflects_engine_retrievability() -> Result<()> {
        let mut col = Collection::new();
        add(&mut col, "mcat::bb::enzymes", Some((1000.0, 5.0)), 3);
        let sec = section("mcat::bb", &[("mcat::bb::enzymes", 1.0)], 0.5);
        let resp = col.compute_readiness(ReadinessRequest {
            search: String::new(),
            mastered_threshold: 0.7,
            sections: vec![sec],
            params: None,
        })?;
        let mem = resp.sections[0].memory.as_ref().unwrap();
        assert!(mem.value > 0.9, "recently reviewed high-stability card -> high retrievability");
        assert_eq!(mem.coverage_pct, 100.0);
        Ok(())
    }

    #[test]
    fn next_actions_rank_uncovered_topic_first() -> Result<()> {
        let mut col = Collection::new();
        // enzymes covered + fresh (low stake); nucleic_acids uncovered (full weight)
        add(&mut col, "mcat::bb::enzymes", Some((1000.0, 5.0)), 3);
        let sec = section("mcat::bb", &[("mcat::bb::enzymes", 0.4), ("mcat::bb::nucleic_acids", 0.6)], 0.6);
        let resp = col.compute_readiness(ReadinessRequest {
            search: String::new(),
            mastered_threshold: 0.7,
            sections: vec![sec],
            params: None,
        })?;
        assert!(!resp.next_actions.is_empty(), "there is always a next action");
        let top = &resp.next_actions[0];
        assert_eq!(top.action_type, "cover");
        assert_eq!(top.topic_id, "mcat::bb::nucleic_acids");
        // sorted by points-at-stake descending
        for pair in resp.next_actions.windows(2) {
            assert!(pair[0].points_at_stake >= pair[1].points_at_stake);
        }
        Ok(())
    }
}
