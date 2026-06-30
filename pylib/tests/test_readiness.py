# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Python-side test for the in-engine readiness inference (compute_readiness).
Confirms the three scores come back with evidence and the give-up rule fires."""

from anki import stats_pb2
from tests.shared import getEmptyCol


def _add(col, front, tag, answer=None):
    note = col.newNote()
    note["Front"] = front
    note["Back"] = "back"
    note.tags = [tag]
    col.addNote(note)
    if answer is not None:
        col.sched.answerCard(col.sched.getCard(), answer)


def test_compute_readiness_returns_three_scores_and_abstains():
    col = getEmptyCol()
    col.set_config("fsrs", True)
    # one of two BB topics covered -> coverage 50%, reviews far below 200
    _add(col, "enzymes are catalysts", "mcat::bb::enzymes", answer=3)

    section = stats_pb2.ReadinessSectionInput(
        section="BB",
        topic_prefix="mcat::bb",
        topic_weights={"mcat::bb::enzymes": 0.5, "mcat::bb::nucleic_acids": 0.5},
        performance=0.6,
        theta_se=0.3,
        alpha_space=1.0,
        alpha_inter=1.3,
        alpha_test=1.5,
    )
    params = stats_pb2.ReadinessParams(
        b0=-1.0, b_m=1.3, b_p=2.2, b_c=0.5,
        min_graded_reviews=200, min_coverage=0.5, max_irt_se=0.5,
    )

    resp = col._backend.compute_readiness(
        search="", mastered_threshold=0.7, sections=[section], params=params
    )

    assert len(resp.sections) == 1
    s = resp.sections[0]
    # all three scores present, each with a range
    assert s.memory.range_hi >= s.memory.range_lo
    assert 0.0 <= s.performance.value <= 1.0
    # give-up rule: not enough graded reviews -> readiness withheld, reasons listed
    assert s.readiness.abstained
    assert any("graded reviews" in m for m in s.readiness.missing)
    # total abstains when a section abstains (honesty rule)
    assert resp.total.abstained
    # next-best action points at the uncovered topic (max points-at-stake)
    assert s.next_best_topic == "mcat::bb::nucleic_acids"
