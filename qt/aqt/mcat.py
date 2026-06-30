# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""In-app MCAT readiness screen (fork addition).

Renders the three SEPARATE scores (Memory, Performance, Readiness) per section,
each with a range and its evidence, by calling the in-engine `compute_readiness`
RPC. Memory is measured live from the user's reviews; Performance comes from a
short in-app question check the user takes here (stored per section), so the
number is genuinely theirs. The give-up rule is honored and explained.
"""
from __future__ import annotations

import random

import aqt
import aqt.main
from aqt.qt import *
from aqt.webview import AnkiWebView, AnkiWebViewKind

# --- AAMC outline weights (section -> {topic_id: within-section weight}) -------
OUTLINE: dict[str, dict[str, float]] = {
    "BB": {
        "mcat::bb::amino_acids_proteins": 0.16, "mcat::bb::enzymes": 0.14,
        "mcat::bb::nucleic_acids": 0.12, "mcat::bb::metabolism": 0.14,
        "mcat::bb::cell_biology": 0.12, "mcat::bb::microbiology": 0.06,
        "mcat::bb::genetics": 0.10, "mcat::bb::organ_systems": 0.16,
    },
    "CP": {
        "mcat::cp::atomic_structure": 0.08, "mcat::cp::bonding": 0.08,
        "mcat::cp::thermodynamics": 0.12, "mcat::cp::kinetics": 0.12,
        "mcat::cp::acids_bases": 0.12, "mcat::cp::electrochemistry": 0.08,
        "mcat::cp::mechanics": 0.12, "mcat::cp::electrostatics_circuits": 0.10,
        "mcat::cp::optics_sound": 0.08, "mcat::cp::organic_chemistry": 0.10,
    },
    "PS": {
        "mcat::ps::sensation_perception": 0.12, "mcat::ps::learning_memory": 0.14,
        "mcat::ps::cognition_language": 0.12, "mcat::ps::motivation_emotion": 0.12,
        "mcat::ps::identity_personality": 0.12, "mcat::ps::social_processes": 0.14,
        "mcat::ps::social_structure": 0.12, "mcat::ps::demographics_inequality": 0.12,
    },
    "CARS": {
        "mcat::cars::foundations_comprehension": 0.30,
        "mcat::cars::reasoning_within_text": 0.30,
        "mcat::cars::reasoning_beyond_text": 0.40,
    },
}
SECTION_NAMES = {"BB": "Biological & Biochemical", "CP": "Chemical & Physical",
                 "PS": "Psych / Social", "CARS": "Critical Analysis & Reasoning"}

# Readiness mapping coefficients (offline-fit; not yet field-calibrated).
COEFFS = {"b0": -1.0, "b_m": 1.3, "b_p": 2.2, "b_c": 0.5}

# --- In-app question check: real MCQs (front, options, correct_index) ---------
QUESTIONS: dict[str, list[tuple[str, list[str], int]]] = {
    "BB": [
        ("Which set of amino acids is basic (positively charged) at physiological pH?",
         ["Asp, Glu, Ser", "Lys, Arg, His", "Phe, Trp, Tyr", "Gly, Ala, Val"], 1),
        ("A competitive inhibitor changes which kinetic parameters?",
         ["Increases Km, Vmax unchanged", "Decreases Vmax, Km unchanged",
          "Both increase", "Both decrease"], 0),
        ("Net ATP produced directly by glycolysis per glucose?",
         ["0", "2", "4", "36"], 1),
        ("Where does the citric acid cycle occur?",
         ["Cytosol", "Mitochondrial matrix", "Nucleus", "Smooth ER"], 1),
    ],
    "CP": [
        ("For a spontaneous process, the sign of delta-G is?",
         ["Positive", "Zero", "Negative", "Undefined"], 2),
        ("Henderson-Hasselbalch: pH = pKa + log(?)",
         ["[HA]/[A-]", "[A-]/[HA]", "[H+]/[OH-]", "Kw/[H+]"], 1),
        ("Ohm's law is?",
         ["P = IV", "V = IR", "F = ma", "Q = mc dT"], 1),
        ("A catalyst affects a reaction by?",
         ["Lowering activation energy", "Changing delta-G",
          "Shifting equilibrium", "Increasing yield"], 0),
    ],
    "PS": [
        ("Operant conditioning shapes behavior primarily through?",
         ["Involuntary association", "Consequences (reinforcement/punishment)",
          "Observation only", "Genetic priming"], 1),
        ("Weber's law states the just-noticeable difference is?",
         ["Constant in absolute terms", "A constant proportion of stimulus intensity",
          "Independent of intensity", "Always 10%"], 1),
        ("The bystander effect predicts that helping behavior?",
         ["Increases with more people present", "Decreases with more people present",
          "Is unaffected by others", "Only applies online"], 1),
    ],
    "CARS": [
        ("'Reasoning beyond the text' tasks ask you to?",
         ["Recall a memorized fact", "Apply/extrapolate the author's argument to a new context",
          "Define vocabulary", "Summarize the first paragraph"], 1),
        ("The best support for a CARS inference is?",
         ["Outside knowledge", "Evidence stated or implied in the passage",
          "The most extreme option", "The longest option"], 1),
    ],
}

PERF_CONFIG_KEY = "mcat_perf"  # col config: {section: {"correct": int, "total": int}}


def setup_mcat_menu(mw: aqt.main.AnkiQt) -> None:
    """Add Tools -> MCAT Readiness."""
    action = QAction("MCAT Readiness", mw)
    action.setShortcut(QKeySequence("Ctrl+M"))
    qconnect(action.triggered, lambda: open_readiness(mw))
    mw.form.menuTools.addSeparator()
    mw.form.menuTools.addAction(action)


def open_readiness(mw: aqt.main.AnkiQt) -> None:
    if mw.col is None:
        return
    dlg = MCATReadinessDialog(mw)
    dlg.show()


class MCATReadinessDialog(QDialog):
    def __init__(self, mw: aqt.main.AnkiQt) -> None:
        QDialog.__init__(self, mw, Qt.WindowType.Window)
        self.mw = mw
        self.setWindowTitle("MCAT Readiness")
        self.setMinimumSize(820, 760)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        self.web = AnkiWebView(kind=AnkiWebViewKind.DEFAULT)
        self.web.set_bridge_command(self._on_bridge, self)
        layout.addWidget(self.web)
        self.refresh()

    # ----- engine call -----
    def _compute(self):
        from anki import stats_pb2

        perf = self.mw.col.get_config(PERF_CONFIG_KEY, {}) or {}
        sections = []
        for sec, weights in OUTLINE.items():
            rec = perf.get(sec, {})
            correct, total = rec.get("correct", 0), rec.get("total", 0)
            p_s = (correct / total) if total else 0.0
            theta_se = 1.5 / ((total + 1) ** 0.5)
            sections.append(stats_pb2.ReadinessSectionInput(
                section=sec, topic_prefix=f"mcat::{sec.lower()}", topic_weights=weights,
                performance=p_s, theta_se=theta_se,
                alpha_space=1.0, alpha_inter=1.0, alpha_test=1.0,
            ))
        params = stats_pb2.ReadinessParams(
            b0=COEFFS["b0"], b_m=COEFFS["b_m"], b_p=COEFFS["b_p"], b_c=COEFFS["b_c"],
            min_graded_reviews=200, min_coverage=0.5, max_irt_se=0.5,
        )
        return self.mw.col._backend.compute_readiness(
            search="", mastered_threshold=0.7, sections=sections, params=params)

    def refresh(self) -> None:
        try:
            resp = self._compute()
            html = self._render(resp)
        except Exception as exc:  # never crash the app during a study session
            import traceback

            html = (
                "<div style='font-family:sans-serif;padding:20px'>"
                "<h3>MCAT Readiness could not be computed</h3>"
                f"<pre style='white-space:pre-wrap;opacity:.7'>{traceback.format_exc()}</pre>"
                "<p>Tip: make sure your deck uses <code>mcat::&lt;section&gt;::&lt;topic&gt;</code> "
                "tags (import the demo deck to see it working).</p></div>"
            )
            print("MCAT readiness error:", exc)
        self.web.stdHtml(html, context=self)

    # ----- bridge: quiz buttons -----
    def _on_bridge(self, cmd: str) -> bool:
        if cmd.startswith("quiz:"):
            section = cmd.split(":", 1)[1]
            self._run_quiz(section)
            self.refresh()
        return False

    def _run_quiz(self, section: str) -> None:
        qs = list(QUESTIONS.get(section, []))
        if not qs:
            return
        random.shuffle(qs)
        correct = 0
        for i, (stem, options, ans) in enumerate(qs, 1):
            picked = _ask_mcq(self, f"{SECTION_NAMES[section]} — Q{i}/{len(qs)}", stem, options)
            if picked is None:
                return  # cancelled; don't record a partial run
            if picked == ans:
                correct += 1
        perf = self.mw.col.get_config(PERF_CONFIG_KEY, {}) or {}
        rec = perf.get(section, {"correct": 0, "total": 0})
        rec["correct"] += correct
        rec["total"] += len(qs)
        perf[section] = rec
        self.mw.col.set_config(PERF_CONFIG_KEY, perf)

    # ----- rendering -----
    def _render(self, resp) -> str:
        total = resp.total
        if total.abstained:
            head = (
                "<div class='hero withheld'>"
                "<div class='big'>Projected MCAT: — withheld —</div>"
                f"<div class='sub'>Not enough evidence yet in: <b>{', '.join(total.missing)}</b>.</div>"
                "<div class='why'>This is intentional. A good readiness tool refuses to guess until it "
                "has enough data — at least 200 graded reviews and 50% topic coverage per section, plus a "
                "short question check. Keep reviewing and run the section checks below; sections unlock as "
                "evidence accumulates.</div></div>"
            )
        else:
            head = (
                "<div class='hero ok'>"
                f"<div class='big'>Projected MCAT: {total.value:.0f}</div>"
                f"<div class='sub'>Likely range {total.range_lo:.0f}–{total.range_hi:.0f} "
                f"· confidence {total.confidence}</div></div>"
            )

        # Global "what to do next" — always present, ranked by points-at-stake.
        ICONS = {"cover": "🧭", "review": "🔁", "practice_check": "📝"}
        actions_html = ""
        if resp.next_actions:
            items = ""
            for a in resp.next_actions:
                label = _short(a.topic_id) if a.topic_id else f"{a.section} question check"
                items += (
                    f"<li><span class='atype'>{ICONS.get(a.action_type, '•')} "
                    f"{a.action_type.replace('_', ' ')}</span> "
                    f"<b>{a.section} · {label}</b>"
                    f"<div class='areason'>{a.reason}</div></li>"
                )
            actions_html = (
                "<div class='actions-panel'><div class='ap-title'>Do this next "
                "(highest points at stake)</div><ol>" + items + "</ol></div>"
            )

        rows = []
        for s in resp.sections:
            m, p, r = s.memory, s.performance, s.readiness
            perf = self.mw.col.get_config(PERF_CONFIG_KEY, {}) or {}
            answered = perf.get(s.section, {}).get("total", 0)
            rd = (f"<span class='wthld'>withheld</span><div class='miss'>need {', '.join(r.missing)}</div>"
                  if r.abstained else
                  f"<span class='score'>{r.value:.0f}</span> <span class='rng'>[{r.range_lo:.0f}–{r.range_hi:.0f}]</span>")
            rows.append(f"""
              <div class='card'>
                <div class='sec'>{s.section} · {SECTION_NAMES[s.section]}</div>
                <div class='grid'>
                  <div><div class='lbl'>Memory</div><div class='val'>{m.value:.2f}</div>
                       <div class='ev'>{m.graded_reviews} reviews · {m.coverage_pct:.0f}% covered</div></div>
                  <div><div class='lbl'>Performance</div><div class='val'>{p.value:.2f}</div>
                       <div class='ev'>{answered} check Qs answered</div></div>
                  <div><div class='lbl'>Readiness</div><div class='val'>{rd}</div></div>
                </div>
                <div class='actions'>
                  <button onclick="pycmd('quiz:{s.section}')">Take {s.section} question check</button>
                  <span class='nba'>Next best: {_short(s.next_best_topic)}</span>
                </div>
              </div>""")

        caveat = ("<div class='caveat'>Memory is measured live from your reviews (FSRS). Performance comes from "
                  "your in-app question checks. The section→score mapping is evidence-based but <b>not yet "
                  "calibrated against real MCAT outcomes</b>, so treat absolute numbers as indicative.</div>")

        css = """
          body{font-family:-apple-system,Segoe UI,Roboto,sans-serif;margin:0;padding:18px;}
          .hero{border-radius:14px;padding:20px;margin-bottom:16px;}
          .hero.withheld{background:rgba(200,120,0,.12);border:1px solid rgba(200,120,0,.4);}
          .hero.ok{background:rgba(0,160,90,.12);border:1px solid rgba(0,160,90,.4);}
          .big{font-size:26px;font-weight:700;}
          .sub{opacity:.8;margin-top:4px;}
          .why{margin-top:10px;font-size:13px;opacity:.85;line-height:1.5;}
          .card{border:1px solid rgba(128,128,128,.3);border-radius:12px;padding:14px;margin-bottom:12px;}
          .sec{font-weight:600;margin-bottom:10px;}
          .grid{display:grid;grid-template-columns:1fr 1fr 1fr;gap:12px;}
          .lbl{font-size:11px;text-transform:uppercase;letter-spacing:.04em;opacity:.6;}
          .val{font-size:22px;font-weight:700;margin-top:2px;}
          .ev{font-size:11px;opacity:.65;margin-top:2px;}
          .wthld{color:#c87800;font-weight:700;font-size:16px;}
          .miss{font-size:11px;opacity:.7;}
          .score{font-size:22px;font-weight:700;} .rng{opacity:.6;font-size:13px;}
          .actions{margin-top:12px;display:flex;align-items:center;gap:12px;}
          button{padding:7px 12px;border-radius:8px;border:1px solid rgba(128,128,128,.4);
                 background:rgba(80,140,255,.15);cursor:pointer;font-size:13px;}
          .nba{font-size:12px;opacity:.7;}
          .caveat{margin-top:8px;font-size:12px;opacity:.7;line-height:1.5;border-top:1px solid rgba(128,128,128,.25);padding-top:10px;}
          .actions-panel{border:1px solid rgba(80,140,255,.4);background:rgba(80,140,255,.08);border-radius:12px;padding:14px;margin-bottom:16px;}
          .ap-title{font-weight:700;margin-bottom:8px;}
          .actions-panel ol{margin:0;padding-left:20px;}
          .actions-panel li{margin-bottom:8px;}
          .atype{font-size:11px;text-transform:uppercase;letter-spacing:.03em;opacity:.7;margin-right:6px;}
          .areason{font-size:12px;opacity:.75;line-height:1.4;margin-top:2px;}
        """
        return f"<style>{css}</style>{head}{actions_html}{''.join(rows)}{caveat}"


def _short(topic_id: str) -> str:
    return topic_id.split("::")[-1].replace("_", " ") if topic_id else "—"


def _ask_mcq(parent, title: str, stem: str, options: list[str]) -> int | None:
    """Modal single-question MCQ. Returns picked index or None if cancelled."""
    dlg = QDialog(parent)
    dlg.setWindowTitle(title)
    dlg.setMinimumWidth(560)
    lay = QVBoxLayout(dlg)
    q = QLabel(stem)
    q.setWordWrap(True)
    q.setStyleSheet("font-size:15px;font-weight:600;margin-bottom:8px;")
    lay.addWidget(q)
    picked = {"i": None}

    def choose(i: int) -> None:
        picked["i"] = i
        dlg.accept()

    for i, opt in enumerate(options):
        b = QPushButton(opt)
        b.setStyleSheet("text-align:left;padding:8px;")
        qconnect(b.clicked, lambda _=False, idx=i: choose(idx))
        lay.addWidget(b)
    dlg.exec()
    return picked["i"]
