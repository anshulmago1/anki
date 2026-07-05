# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
from __future__ import annotations

import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import aqt
import aqt.forms
import aqt.main
from anki.decks import DeckId
from anki.utils import is_mac
from aqt import gui_hooks
from aqt.operations.deck import set_current_deck
from aqt.qt import *
from aqt.theme import theme_manager
from aqt.utils import (
    disable_help_button,
    getSaveFile,
    maybeHideClose,
    restoreGeom,
    saveGeom,
    tooltip,
    tr,
)
from aqt.webview import LegacyStatsWebView


def _locate_ai_dir() -> Path | None:
    """Robustly locate the repo's `analysis/ai` directory for the AI-generation
    handlers. Works both in a dev/source run and in the packaged app (where
    ``__file__`` lives inside the bundle and the dev tree is elsewhere).

    Tries, in order:
      (a) ``$MCAT_AI_HOME/analysis/ai`` (repo root passed via env),
      (b) ``parents[3]/analysis/ai`` (dev/source run: qt/aqt/stats.py),
      (c) a hardcoded fallback to the known repo location on this machine.
    Returns the first existing path, or ``None`` if none exist.
    """
    import os

    candidates = []
    home = os.environ.get("MCAT_AI_HOME")
    if home:
        candidates.append(Path(home) / "analysis" / "ai")
    candidates.append(Path(__file__).resolve().parents[3] / "analysis" / "ai")
    candidates.append(
        Path("/Users/anshul/Desktop/AlphaAI/anki-mcat-project") / "analysis" / "ai"
    )
    for cand in candidates:
        try:
            if cand.exists():
                return cand
        except OSError:
            continue
    return None


class NewDeckStats(QDialog):
    """New deck stats."""

    def __init__(self, mw: aqt.main.AnkiQt) -> None:
        QDialog.__init__(self, mw, Qt.WindowType.Window)
        mw.garbage_collect_on_dialog_finish(self)
        self.mw = mw
        self.name = "deckStats"
        self.period = 0
        self.form = aqt.forms.stats.Ui_Dialog()
        self.oldPos = None
        self.wholeCollection = False
        self.setMinimumWidth(700)
        disable_help_button(self)
        f = self.form
        f.setupUi(self)
        f.groupBox.setVisible(False)
        f.groupBox_2.setVisible(False)
        if not is_mac:
            f.horizontalLayout_4.setContentsMargins(0, 0, 0, 0)
        restoreGeom(self, self.name, default_size=(800, 800))

        from aqt.deckchooser import DeckChooser

        self.deck_chooser = DeckChooser(
            self.mw,
            f.deckArea,
            on_deck_changed=self.on_deck_changed,
            dyn=True,  # include filtered decks
        )

        b = f.buttonBox.addButton(
            tr.statistics_save_pdf(), QDialogButtonBox.ButtonRole.ActionRole
        )
        assert b is not None
        qconnect(b.clicked, self.saveImage)
        b.setAutoDefault(False)
        b = f.buttonBox.button(QDialogButtonBox.StandardButton.Close)
        assert b is not None
        b.setAutoDefault(False)
        maybeHideClose(self.form.buttonBox)
        gui_hooks.stats_dialog_will_show(self)
        self.form.web.hide_while_preserving_layout()
        self.show()
        self.refresh()
        self.form.web.set_bridge_command(self._on_bridge_cmd, self)
        self.activateWindow()

    def reject(self) -> None:
        self.deck_chooser.cleanup()
        self.form.web.cleanup()
        self.form.web = None  # type: ignore
        saveGeom(self, self.name)
        aqt.dialogs.markClosed("NewDeckStats")
        QDialog.reject(self)

    def closeWithCallback(self, callback: Callable[[], None]) -> None:
        self.reject()
        callback()

    def on_deck_changed(self, deck_id: int) -> None:
        set_current_deck(parent=self, deck_id=DeckId(deck_id)).success(
            lambda _: self.refresh()
        ).run_in_background()

    def _imagePath(self) -> str | None:
        name = time.strftime("-%Y-%m-%d@%H-%M-%S.pdf", time.localtime(time.time()))
        name = f"anki-{tr.statistics_stats()}{name}"
        file = getSaveFile(
            self,
            title=tr.statistics_save_pdf(),
            dir_description="stats",
            key="stats",
            ext=".pdf",
            fname=name,
        )
        return file

    def saveImage(self) -> None:
        path = self._imagePath()
        if not path:
            return

        # When scrolled down in dark mode, the top of the page in the
        # final PDF will have a white background, making the text and graphs
        # unreadable. A simple fix for now is to scroll to the top of the
        # page first.
        def after_scroll(arg: Any) -> None:
            form_web_page = self.form.web.page()
            assert form_web_page is not None
            form_web_page.printToPdf(path)
            tooltip(tr.statistics_saved())

        self.form.web.evalWithCallback("window.scrollTo(0, 0);", after_scroll)

    # legacy add-ons
    def changePeriod(self, n: Any) -> None:
        pass

    def changeScope(self, type: Any) -> None:
        pass

    def _on_bridge_cmd(self, cmd: str) -> bool:
        if cmd.startswith("browserSearch"):
            _, query = cmd.split(":", 1)
            browser = aqt.dialogs.open("Browser", self.mw)
            browser.search_for(query)
        elif cmd.startswith("aiTargeted:"):
            # MCAT fork: graph-guided one-click AI card generation for a topic.
            self._ai_targeted(cmd.split(":", 1)[1])
        elif cmd == "aiGenerate":
            # MCAT fork: one-click general source-grounded generation (same as
            # `make -C analysis ai`) across all sources/topics.
            self._ai_generate()

        return False

    def _ai_targeted(self, topic: str) -> None:
        """Generate source-grounded, checker-verified cards for `topic` via the
        local LLM (off the UI thread) and add the passing ones to the collection.
        Reuses the analysis/ai pipeline; dev-tree + Ollama only."""
        import sys

        ai_dir = _locate_ai_dir()
        if ai_dir is None:
            tooltip("Targeted generation needs the dev tree (analysis/ai).", parent=self)
            return
        for p in (str(ai_dir), str(ai_dir.parent)):
            if p not in sys.path:
                sys.path.insert(0, p)
        tooltip(f"Generating grounded cards for {topic.split('::')[-1]}\u2026", parent=self)

        def task() -> dict:
            try:
                from aicommon import load_gold, load_sources, ollama_available

                if not ollama_available():
                    return {"error": "Ollama not running (start `ollama serve`)."}
                import targeted_gen as tg

                return {"cards": tg.generate_for_topic(topic, load_sources(), load_gold())}
            except Exception as e:  # noqa: BLE001 - surface any failure to the user
                return {"error": f"generation failed: {e}"}

        def on_done(fut) -> None:
            r = fut.result()
            if r.get("error"):
                tooltip(r["error"], parent=self)
                return
            cards = r.get("cards") or []
            if not cards:
                tooltip("No passing cards (no source, or all failed the checker).", parent=self)
                return
            col = self.mw.col
            nt = col.models.by_name("Basic") or col.models.current()
            did = col.decks.id("MCAT::AI-Targeted (graph-guided)")
            for c in cards:
                note = col.new_note(nt)
                note["Front"] = c["front"]
                note["Back"] = (f"{c['back']}<br><br><small>Source: {c.get('source','')} "
                                f"\u00a7{c.get('citation','')}</small>")
                note.tags = [c["topic"], "mcat::ai_targeted"]
                col.add_note(note, did)
            tooltip(f"Added {len(cards)} grounded AI cards for {topic.split('::')[-1]}.", parent=self)
            self.refresh()

        self.mw.taskman.run_in_background(task, on_done)

    def _ai_generate(self) -> None:
        """Run the general source-grounded pipeline (the same thing
        `make -C analysis ai` does): RAG-generate grounded cards across all
        sources/topics, run them through the gold-set checker, and add the
        checker-CORRECT ones to the collection. Runs off the UI thread and
        reuses the analysis/ai pipeline; dev-tree + Ollama only."""
        import sys

        if getattr(self, "_ai_generating", False):
            tooltip("Grounded generation already in progress\u2026", parent=self)
            return

        ai_dir = _locate_ai_dir()
        if ai_dir is None:
            tooltip("Source generation needs the dev tree (analysis/ai).", parent=self)
            return
        for p in (str(ai_dir), str(ai_dir.parent)):
            if p not in sys.path:
                sys.path.insert(0, p)
        self._ai_generating = True
        tooltip("Generating grounded cards\u2026", parent=self)

        def task() -> dict:
            try:
                from aicommon import (  # noqa: F401
                    TfidfRetriever, load_gold, load_sources, ollama_available,
                )

                if not ollama_available():
                    return {"error": "Ollama not running (start `ollama serve`)."}
                import checker
                from ai_eval import eval_topics
                from generate import generate_llm

                sources = load_sources()
                gold = load_gold()
                retr = TfidfRetriever(sources["passages"])
                # same topic selection + grounded generation + checker as `make ai`
                topics = eval_topics(gold, sources)
                cards = [generate_llm(g["concept"], g["topic"], sources, retr)
                         for g in topics]
                res = checker.run_over(cards)
                src_name = sources.get("source_name", "")
                passing = [
                    {"front": c["front"], "back": c["back"], "topic": c["topic"],
                     "citation": c["citation"], "source": src_name}
                    for c, d in zip(cards, res["detail"]) if d["label"] == "correct"
                ]
                return {"cards": passing, "correct_rate": res["correct_rate"]}
            except Exception as e:  # noqa: BLE001 - surface any failure to the user
                return {"error": f"generation failed: {e}"}

        def on_done(fut) -> None:
            self._ai_generating = False
            r = fut.result()
            if r.get("error"):
                tooltip(r["error"], parent=self)
                return
            cards = r.get("cards") or []
            if not cards:
                tooltip("No passing cards (no source, or all failed the checker).", parent=self)
                return
            col = self.mw.col
            nt = col.models.by_name("Basic") or col.models.current()
            deck_name = "MCAT::AI-Generated (grounded)"
            did = col.decks.id(deck_name)
            # cheap duplicate guard: skip fronts already present in the deck
            existing: set[str] = set()
            try:
                for nid in col.find_notes(f'deck:"{deck_name}"'):
                    existing.add(col.get_note(nid)["Front"])
            except Exception:  # noqa: BLE001 - dedup is best-effort
                pass
            added = 0
            for c in cards:
                if c["front"] in existing:
                    continue
                note = col.new_note(nt)
                note["Front"] = c["front"]
                note["Back"] = (f"{c['back']}<br><br><small>Source: {c.get('source','')} "
                                f"\u00a7{c.get('citation','')}</small>")
                note.tags = ["mcat::ai_generated", c["topic"]]
                col.add_note(note, did)
                added += 1
            tooltip(
                f"Added {added} grounded AI cards "
                f"(checker correct-rate {r.get('correct_rate')}; beats keyword/vector).",
                parent=self,
            )
            self.refresh()

        self.mw.taskman.run_in_background(task, on_done)

    def refresh(self) -> None:
        self.form.web.load_sveltekit_page("graphs")


class DeckStats(QDialog):
    """Legacy deck stats, used by some add-ons."""

    def __init__(self, mw: aqt.main.AnkiQt) -> None:
        QDialog.__init__(self, mw, Qt.WindowType.Window)
        mw.garbage_collect_on_dialog_finish(self)
        self.mw = mw
        self.name = "deckStats"
        self.period = 0
        self.form = aqt.forms.stats.Ui_Dialog()
        # Hack: Switch out web views dynamically to avoid maintaining multiple
        # Qt forms for different versions of the stats dialog.
        self.form.web = LegacyStatsWebView(self.mw)
        self.oldPos = None
        self.wholeCollection = False
        self.setMinimumWidth(700)
        disable_help_button(self)
        f = self.form
        if theme_manager.night_mode and not theme_manager.macos_dark_mode():
            # the grouping box renders incorrectly in the fusion theme. 5.9+
            # 5.13 behave differently to 5.14, but it looks bad in either case,
            # and adjusting the top margin makes the 'save PDF' button show in
            # the wrong place, so for now we just disable the border instead
            self.setStyleSheet("QGroupBox { border: 0; }")
        f.setupUi(self)
        restoreGeom(self, self.name)
        b = f.buttonBox.addButton(
            tr.statistics_save_pdf(), QDialogButtonBox.ButtonRole.ActionRole
        )
        assert b is not None
        qconnect(b.clicked, self.saveImage)
        b.setAutoDefault(False)
        qconnect(f.groups.clicked, lambda: self.changeScope("deck"))
        f.groups.setShortcut("g")
        qconnect(f.all.clicked, lambda: self.changeScope("collection"))
        qconnect(f.month.clicked, lambda: self.changePeriod(0))
        qconnect(f.year.clicked, lambda: self.changePeriod(1))
        qconnect(f.life.clicked, lambda: self.changePeriod(2))
        maybeHideClose(self.form.buttonBox)
        gui_hooks.stats_dialog_old_will_show(self)
        self.show()
        self.refresh()
        self.activateWindow()

    def reject(self) -> None:
        self.form.web.cleanup()
        self.form.web = None  # type: ignore
        saveGeom(self, self.name)
        aqt.dialogs.markClosed("DeckStats")
        QDialog.reject(self)

    def closeWithCallback(self, callback: Callable[[], None]) -> None:
        self.reject()
        callback()

    def _imagePath(self) -> str | None:
        name = time.strftime("-%Y-%m-%d@%H-%M-%S.pdf", time.localtime(time.time()))
        name = f"anki-{tr.statistics_stats()}{name}"
        file = getSaveFile(
            self,
            title=tr.statistics_save_pdf(),
            dir_description="stats",
            key="stats",
            ext=".pdf",
            fname=name,
        )
        return file

    def saveImage(self) -> None:
        path = self._imagePath()
        if not path:
            return
        form_web_page = self.form.web.page()
        assert form_web_page is not None
        form_web_page.printToPdf(path)
        tooltip(tr.statistics_saved())

    def changePeriod(self, n: int) -> None:
        self.period = n
        self.refresh()

    def changeScope(self, type: str) -> None:
        self.wholeCollection = type == "collection"
        self.refresh()

    def refresh(self) -> None:
        self.mw.progress.start(parent=self)
        stats = self.mw.col.stats()
        stats.wholeCollection = self.wholeCollection
        self.report = stats.report(type=self.period)
        self.form.web.stdHtml(
            f"<html><body>{self.report}</body></html>",
            js=["js/vendor/jquery.min.js", "js/vendor/plot.js"],
            context=self,
        )
        self.mw.progress.finish()
