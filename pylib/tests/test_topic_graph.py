# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Python-side test for the in-engine knowledge graph (topic_graph). Confirms the
binding is exposed to pylib (the desktop webview calls it) and that the
prerequisite learning_ready gate + recommended path behave as specified."""

from anki import stats_pb2
from tests.shared import getEmptyCol


def _add(col, front, tag, answers=()):
    note = col.newNote()
    note["Front"] = front
    note["Back"] = "back"
    note.tags = [tag]
    col.addNote(note)
    for a in answers:
        col.sched.answerCard(col.sched.getCard(), a)


def test_topic_graph_learning_ready_and_path():
    col = getEmptyCol()
    col.set_config("fsrs", True)
    # A (proteins) is a prerequisite of B (enzymes). Both get one review so they
    # are covered; A answered "easy", B answered "again" (weak).
    _add(col, "amino acid structure", "mcat::bb::amino_acids_proteins", answers=(4,))
    _add(col, "enzymes are catalysts", "mcat::bb::enzymes", answers=(1,))

    nodes = [
        stats_pb2.TopicGraphNode(
            id="mcat::bb::amino_acids_proteins", section="BB",
            label="Amino acids", weight=0.5,
        ),
        stats_pb2.TopicGraphNode(
            id="mcat::bb::enzymes", section="BB", label="Enzymes", weight=0.5,
        ),
    ]
    edges = [
        stats_pb2.TopicGraphEdge(
            **{"from": "mcat::bb::amino_acids_proteins", "to": "mcat::bb::enzymes"}
        )
    ]

    resp = col._backend.topic_graph(
        search="",
        topic_prefix="mcat",
        mastered_threshold=0.7,
        mastery_threshold=0.7,
        nodes=nodes,
        prereq_edges=edges,
    )

    assert len(resp.nodes) == 2
    by_id = {n.id: n for n in resp.nodes}
    # both topics have cards -> covered
    assert by_id["mcat::bb::enzymes"].covered
    assert by_id["mcat::bb::amino_acids_proteins"].covered
    # the prerequisite node has no prereqs of its own -> always learning_ready
    assert by_id["mcat::bb::amino_acids_proteins"].learning_ready
    # recommended path only contains unmastered nodes; every entry is a real node
    assert set(resp.recommended_path).issubset(set(by_id.keys()))
    # if the prerequisite is not yet mastered, the dependent lists it as blocked
    b = by_id["mcat::bb::enzymes"]
    if not by_id["mcat::bb::amino_acids_proteins"].mastered:
        assert not b.learning_ready
        assert "mcat::bb::amino_acids_proteins" in b.unmet_prereqs
    # in the recommended path the prerequisite precedes the dependent
    path = resp.recommended_path
    if "mcat::bb::enzymes" in path and "mcat::bb::amino_acids_proteins" in path:
        assert path.index("mcat::bb::amino_acids_proteins") < path.index("mcat::bb::enzymes")


def test_topic_graph_uncovered_carries_full_weight():
    col = getEmptyCol()
    col.set_config("fsrs", True)
    nodes = [
        stats_pb2.TopicGraphNode(
            id="mcat::cp::optics_sound", section="CP", label="Optics", weight=0.3,
        )
    ]
    resp = col._backend.topic_graph(
        search="", topic_prefix="mcat", mastered_threshold=0.7,
        mastery_threshold=0.7, nodes=nodes, prereq_edges=[],
    )
    n = resp.nodes[0]
    assert not n.covered
    assert not n.mastered
    assert abs(n.points_at_stake - 0.3) < 1e-6
    # a lone uncovered node with no prereqs is still the next thing to cover
    assert n.learning_ready
    assert resp.recommended_path == ["mcat::cp::optics_sound"]
