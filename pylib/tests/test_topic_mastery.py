# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Python-side test for the MCAT mastery-query Rust change (Speedrun 7a:
'1 test that calls your change from Python'). Also checks the read-only query
leaves the collection usable and does not disturb undo state."""

from tests.shared import getEmptyCol


def _add_tagged(col, front, tags, answer=None):
    note = col.newNote()
    note["Front"] = front
    note["Back"] = "back"
    note.tags = list(tags)
    col.addNote(note)
    card = note.cards()[0]
    if answer is not None:
        col.sched.answerCard(col.sched.getCard(), answer)
    return note, card


def test_topic_mastery_groups_by_tag():
    col = getEmptyCol()
    _add_tagged(col, "enzyme kinetics", ["mcat::bb::enzymes"], answer=3)
    _add_tagged(col, "michaelis menten", ["mcat::bb::enzymes"], answer=3)
    _add_tagged(col, "rate law", ["mcat::cp::kinetics"], answer=3)
    _add_tagged(col, "untagged fact", [])  # must NOT appear

    resp = col._backend.topic_mastery(
        search="", topic_prefix="mcat", mastered_threshold=0.5
    )
    # the generated binding unwraps the single repeated field, returning the list
    topics = {t.topic_id: t for t in resp}

    # only the two tagged topics are present; the untagged card is excluded
    assert set(topics) == {"mcat::bb::enzymes", "mcat::cp::kinetics"}
    assert topics["mcat::bb::enzymes"].card_count == 2
    assert topics["mcat::cp::kinetics"].card_count == 1
    # each answered card recorded a review -> evidence volume is non-zero
    assert topics["mcat::bb::enzymes"].graded_reviews >= 2
    # mean retrievability is a valid probability
    assert 0.0 <= topics["mcat::cp::kinetics"].mean_retrievability <= 1.0


def test_topic_mastery_is_read_only_and_undo_safe():
    col = getEmptyCol()
    note, _ = _add_tagged(col, "amino acid", ["mcat::bb::amino_acids_proteins"], answer=3)
    before = col.card_count()
    undo_before = col.undo_status().undo

    col._backend.topic_mastery(search="", topic_prefix="mcat", mastered_threshold=0.9)

    # the query mutates nothing: card count and undo state are unchanged,
    # and the collection is still fully usable afterwards.
    assert col.card_count() == before
    assert col.undo_status().undo == undo_before
    assert col.get_note(note.id) is not None
