<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

MCAT cue-diagnostics knowledge graph, rendered inside the Statistics page.
Calls the in-engine TopicGraph RPC (per-node mastery measured from live cards +
prerequisite "learning_ready" gate + points-at-stake + a recommended traversal
path). Nodes are AAMC content categories; edges are prerequisites (solid, with
arrows) and related concepts (dashed). Colour encodes mastery state. Clicking a
node opens the Browser filtered to that topic's cards (click-to-study).

The graph structure mirrors data/knowledge_graph.json (desktop-only surface, so
a single copy; the engine measures the mastery).
-->
<script lang="ts">
    import { topicGraph } from "@generated/backend";
    import type { TopicGraphNodeStat } from "@generated/anki/stats_pb";
    import { bridgeCommand } from "@tslib/bridgecommand";

    type Node = { id: string; section: string; label: string; weight: number };
    type Edge = { from: string; to: string };

    const NODES: Node[] = [
        { id: "mcat::bb::amino_acids_proteins", section: "BB", label: "Amino acids & proteins", weight: 0.16 },
        { id: "mcat::bb::enzymes", section: "BB", label: "Enzymes & kinetics", weight: 0.14 },
        { id: "mcat::bb::nucleic_acids", section: "BB", label: "Nucleic acids", weight: 0.12 },
        { id: "mcat::bb::metabolism", section: "BB", label: "Metabolism", weight: 0.14 },
        { id: "mcat::bb::cell_biology", section: "BB", label: "Cell biology", weight: 0.12 },
        { id: "mcat::bb::microbiology", section: "BB", label: "Microbiology", weight: 0.06 },
        { id: "mcat::bb::genetics", section: "BB", label: "Genetics", weight: 0.1 },
        { id: "mcat::bb::organ_systems", section: "BB", label: "Organ systems", weight: 0.16 },
        { id: "mcat::cp::atomic_structure", section: "CP", label: "Atomic structure", weight: 0.08 },
        { id: "mcat::cp::bonding", section: "CP", label: "Bonding", weight: 0.08 },
        { id: "mcat::cp::thermodynamics", section: "CP", label: "Thermodynamics", weight: 0.12 },
        { id: "mcat::cp::kinetics", section: "CP", label: "Kinetics & equilibrium", weight: 0.12 },
        { id: "mcat::cp::acids_bases", section: "CP", label: "Acids & bases", weight: 0.12 },
        { id: "mcat::cp::electrochemistry", section: "CP", label: "Electrochemistry", weight: 0.08 },
        { id: "mcat::cp::mechanics", section: "CP", label: "Mechanics & energy", weight: 0.12 },
        { id: "mcat::cp::electrostatics_circuits", section: "CP", label: "Electrostatics & circuits", weight: 0.1 },
        { id: "mcat::cp::optics_sound", section: "CP", label: "Light & sound", weight: 0.08 },
        { id: "mcat::cp::organic_chemistry", section: "CP", label: "Organic chemistry", weight: 0.1 },
        { id: "mcat::ps::sensation_perception", section: "PS", label: "Sensation & perception", weight: 0.12 },
        { id: "mcat::ps::learning_memory", section: "PS", label: "Learning & memory", weight: 0.14 },
        { id: "mcat::ps::cognition_language", section: "PS", label: "Cognition & language", weight: 0.12 },
        { id: "mcat::ps::motivation_emotion", section: "PS", label: "Motivation & emotion", weight: 0.12 },
        { id: "mcat::ps::identity_personality", section: "PS", label: "Identity & personality", weight: 0.12 },
        { id: "mcat::ps::social_processes", section: "PS", label: "Social processes", weight: 0.14 },
        { id: "mcat::ps::social_structure", section: "PS", label: "Social structure", weight: 0.12 },
        { id: "mcat::ps::demographics_inequality", section: "PS", label: "Demographics & inequality", weight: 0.12 },
        { id: "mcat::cars::foundations_comprehension", section: "CARS", label: "Comprehension", weight: 0.3 },
        { id: "mcat::cars::reasoning_within_text", section: "CARS", label: "Reasoning within text", weight: 0.3 },
        { id: "mcat::cars::reasoning_beyond_text", section: "CARS", label: "Reasoning beyond text", weight: 0.4 },
    ];

    const PREREQ: Edge[] = [
        { from: "mcat::bb::amino_acids_proteins", to: "mcat::bb::enzymes" },
        { from: "mcat::bb::enzymes", to: "mcat::bb::metabolism" },
        { from: "mcat::bb::nucleic_acids", to: "mcat::bb::genetics" },
        { from: "mcat::bb::cell_biology", to: "mcat::bb::microbiology" },
        { from: "mcat::bb::cell_biology", to: "mcat::bb::organ_systems" },
        { from: "mcat::bb::metabolism", to: "mcat::bb::organ_systems" },
        { from: "mcat::cp::atomic_structure", to: "mcat::cp::bonding" },
        { from: "mcat::cp::bonding", to: "mcat::cp::acids_bases" },
        { from: "mcat::cp::bonding", to: "mcat::cp::organic_chemistry" },
        { from: "mcat::cp::thermodynamics", to: "mcat::cp::kinetics" },
        { from: "mcat::cp::acids_bases", to: "mcat::cp::electrochemistry" },
        { from: "mcat::cp::mechanics", to: "mcat::cp::electrostatics_circuits" },
        { from: "mcat::cp::mechanics", to: "mcat::cp::optics_sound" },
        { from: "mcat::ps::sensation_perception", to: "mcat::ps::cognition_language" },
        { from: "mcat::ps::learning_memory", to: "mcat::ps::cognition_language" },
        { from: "mcat::ps::cognition_language", to: "mcat::ps::motivation_emotion" },
        { from: "mcat::ps::identity_personality", to: "mcat::ps::social_processes" },
        { from: "mcat::ps::social_processes", to: "mcat::ps::social_structure" },
        { from: "mcat::ps::social_structure", to: "mcat::ps::demographics_inequality" },
        { from: "mcat::cars::foundations_comprehension", to: "mcat::cars::reasoning_within_text" },
        { from: "mcat::cars::reasoning_within_text", to: "mcat::cars::reasoning_beyond_text" },
    ];

    const RELATED: Edge[] = [
        { from: "mcat::bb::enzymes", to: "mcat::cp::kinetics" },
        { from: "mcat::bb::metabolism", to: "mcat::cp::thermodynamics" },
        { from: "mcat::bb::amino_acids_proteins", to: "mcat::cp::acids_bases" },
        { from: "mcat::bb::organ_systems", to: "mcat::cp::electrostatics_circuits" },
        { from: "mcat::bb::organ_systems", to: "mcat::ps::sensation_perception" },
        { from: "mcat::cp::organic_chemistry", to: "mcat::bb::amino_acids_proteins" },
        { from: "mcat::cp::optics_sound", to: "mcat::ps::sensation_perception" },
        { from: "mcat::ps::learning_memory", to: "mcat::bb::organ_systems" },
    ];

    const SECTIONS = ["BB", "CP", "PS", "CARS"];
    const SECTION_LABEL: Record<string, string> = {
        BB: "Bio/Biochem",
        CP: "Chem/Physics",
        PS: "Psych/Social",
        CARS: "CARS",
    };

    // layout: one column per section; within a column, y-order by prerequisite depth.
    const COL_W = 250;
    const ROW_H = 78;
    const NODE_R = 22;
    const TOP = 50;
    const WIDTH = SECTIONS.length * COL_W;

    function prereqDepth(): Record<string, number> {
        const preds: Record<string, string[]> = {};
        for (const n of NODES) preds[n.id] = [];
        for (const e of PREREQ) preds[e.to]?.push(e.from);
        const depth: Record<string, number> = {};
        const visit = (id: string): number => {
            if (depth[id] !== undefined) return depth[id];
            const ps = preds[id] ?? [];
            depth[id] = ps.length ? 1 + Math.max(...ps.map(visit)) : 0;
            return depth[id];
        };
        for (const n of NODES) visit(n.id);
        return depth;
    }

    const depth = prereqDepth();
    const pos: Record<string, { x: number; y: number }> = {};
    for (let si = 0; si < SECTIONS.length; si++) {
        const sec = SECTIONS[si];
        const inSec = NODES.filter((n) => n.section === sec).sort(
            (a, b) => depth[a.id] - depth[b.id] || a.label.localeCompare(b.label),
        );
        inSec.forEach((n, i) => {
            pos[n.id] = { x: si * COL_W + COL_W / 2, y: TOP + i * ROW_H };
        });
    }
    const maxRows = Math.max(
        ...SECTIONS.map((s) => NODES.filter((n) => n.section === s).length),
    );
    const HEIGHT = TOP + maxRows * ROW_H + 20;

    let stats: Record<string, TopicGraphNodeStat> = {};
    let path: string[] = [];
    let pathIndex: Record<string, number> = {};
    let selected: TopicGraphNodeStat | null = null;
    let errored = false;
    let loaded = false;
    // Collapsed by default so the Statistics page looks like normal Anki; the
    // engine query only runs the first time the student opens the graph.
    let collapsed = true;
    let started = false;

    async function load(): Promise<void> {
        try {
            const resp = await topicGraph({
                search: "",
                topicPrefix: "mcat",
                masteredThreshold: 0.7,
                masteryThreshold: 0.7,
                nodes: NODES,
                prereqEdges: PREREQ,
            });
            for (const n of resp.nodes) stats[n.id] = n;
            path = resp.recommendedPath;
            path.forEach((id, i) => (pathIndex[id] = i));
            loaded = true;
        } catch (e) {
            console.error("topic graph failed", e);
            errored = true;
        }
    }

    function toggle(): void {
        collapsed = !collapsed;
        if (!collapsed && !started) {
            started = true;
            void load();
        }
    }

    type State = "mastered" | "ready" | "blocked" | "uncovered" | "unknown";
    function nodeState(id: string): State {
        const s = stats[id];
        if (!s) return "unknown";
        if (s.mastered) return "mastered";
        if (!s.covered) return "uncovered";
        return s.learningReady ? "ready" : "blocked";
    }
    const COLORS: Record<State, string> = {
        mastered: "#2e9e5b",
        ready: "#e0a52e",
        blocked: "#c0504d",
        uncovered: "#9aa0a6",
        unknown: "#c8ccd0",
    };

    function labelOf(id: string): string {
        return NODES.find((n) => n.id === id)?.label ?? id;
    }
    function study(id: string): void {
        bridgeCommand(`browserSearch:tag:${id}`);
    }
    function pct(x: number | undefined): string {
        return x === undefined ? "\u2014" : `${Math.round(x * 100)}%`;
    }

    $: recommended = path.map((id) => ({ id, label: labelOf(id), state: nodeState(id) }));
</script>

<div class="kg" class:collapsed>
    <button class="kg-toggle" on:click={toggle} aria-expanded={!collapsed}>
        <span class="chev">{collapsed ? "\u25B8" : "\u25BE"}</span>
        {collapsed ? "Show knowledge graph" : "Hide knowledge graph"}
        <span class="kg-toggle-sub">cue-diagnostics map &middot; next concept to master</span>
    </button>

    {#if !collapsed}
        <div class="kg-legend">
            <span><i style="background:{COLORS.mastered}"></i> mastered</span>
            <span><i style="background:{COLORS.ready}"></i> study now</span>
            <span><i style="background:{COLORS.blocked}"></i> locked (prereq)</span>
            <span><i style="background:{COLORS.uncovered}"></i> no cards yet</span>
        </div>

        {#if errored}
            <div class="kg-msg">Knowledge graph unavailable (engine RPC failed).</div>
        {:else if !loaded}
            <div class="kg-msg">Measuring topic mastery&hellip;</div>
        {:else}
        <div class="kg-body">
            <svg viewBox="0 0 {WIDTH} {HEIGHT}" class="kg-svg">
                <defs>
                    <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5"
                        markerWidth="6" markerHeight="6" orient="auto-start-reverse">
                        <path d="M0,0 L10,5 L0,10 z" fill="#9aa0a6" />
                    </marker>
                </defs>

                {#each SECTIONS as sec, si}
                    <text x={si * COL_W + COL_W / 2} y="24" class="kg-col" text-anchor="middle">
                        {SECTION_LABEL[sec]}
                    </text>
                {/each}

                <!-- related edges (dashed, faint) -->
                {#each RELATED as e}
                    {#if pos[e.from] && pos[e.to]}
                        <line x1={pos[e.from].x} y1={pos[e.from].y}
                            x2={pos[e.to].x} y2={pos[e.to].y}
                            class="edge related" />
                    {/if}
                {/each}

                <!-- prerequisite edges (solid, arrow) -->
                {#each PREREQ as e}
                    {#if pos[e.from] && pos[e.to]}
                        <line x1={pos[e.from].x} y1={pos[e.from].y}
                            x2={pos[e.to].x} y2={pos[e.to].y}
                            class="edge prereq" marker-end="url(#arrow)" />
                    {/if}
                {/each}

                <!-- nodes -->
                {#each NODES as n}
                    {#if pos[n.id]}
                        <g class="node" class:sel={selected?.id === n.id}
                            transform="translate({pos[n.id].x},{pos[n.id].y})"
                            on:click={() => (selected = stats[n.id] ?? null)}
                            on:keydown={(e) => (e.key === "Enter" || e.key === " ") && (selected = stats[n.id] ?? null)}
                            role="button" tabindex="0">
                            <circle r={NODE_R} fill={COLORS[nodeState(n.id)]}
                                stroke={pathIndex[n.id] !== undefined ? "#1a73e8" : "#00000022"}
                                stroke-width={pathIndex[n.id] !== undefined ? 3 : 1} />
                            {#if pathIndex[n.id] !== undefined}
                                <circle r="9" cx={NODE_R - 4} cy={-NODE_R + 4} fill="#1a73e8" />
                                <text x={NODE_R - 4} y={-NODE_R + 7} class="kg-badge"
                                    text-anchor="middle">{pathIndex[n.id] + 1}</text>
                            {/if}
                            <text y="1" class="kg-node-r" text-anchor="middle">
                                {stats[n.id]?.covered ? pct(stats[n.id]?.meanRetrievability) : ""}
                            </text>
                            <text y={NODE_R + 14} class="kg-node-lbl" text-anchor="middle">
                                {n.label}
                            </text>
                        </g>
                    {/if}
                {/each}
            </svg>

            <div class="kg-side">
                <div class="kg-panel">
                    <div class="kg-panel-title">Recommended path</div>
                    <ol>
                        {#each recommended.slice(0, 8) as r}
                            <li>
                                <span class="dot" style="background:{COLORS[r.state]}"></span>
                                <button class="linkish" on:click={() => study(r.id)}>{r.label}</button>
                            </li>
                        {/each}
                    </ol>
                    <div class="kg-hint">Prerequisite-respecting, ordered by points at stake.</div>
                </div>

                {#if selected}
                    {@const sel = selected}
                    <div class="kg-panel">
                        <div class="kg-panel-title">{labelOf(sel.id)}</div>
                        <table>
                            <tbody>
                                <tr><td>State</td><td><b>{nodeState(sel.id)}</b></td></tr>
                                <tr><td>Memory (recall)</td><td>{sel.covered ? pct(sel.meanRetrievability) : "no cards"}</td></tr>
                                <tr><td>Cards</td><td>{sel.cardCount} ({sel.masteredCount} mastered)</td></tr>
                                <tr><td>Reviews</td><td>{sel.gradedReviews}</td></tr>
                                <tr><td>Points at stake</td><td>{sel.pointsAtStake.toFixed(3)}</td></tr>
                                {#if sel.unmetPrereqs.length}
                                    <tr><td>Blocked by</td><td>{sel.unmetPrereqs.map(labelOf).join(", ")}</td></tr>
                                {/if}
                            </tbody>
                        </table>
                        <button class="kg-study" on:click={() => study(sel.id)}>
                            Study these cards &rarr;
                        </button>
                    </div>
                {:else}
                    <div class="kg-panel kg-empty">Click a node for its evidence.</div>
                {/if}
            </div>
        </div>
        {/if}
    {/if}
</div>

<style>
    .kg {
        margin: 1em auto;
        max-width: 60em;
        padding: 1em 1.5em;
        border: 1px solid var(--border, #d7dade);
        border-radius: 10px;
        background: var(--canvas-elevated, #fff);
    }
    .kg.collapsed {
        padding: 0.4em 1em;
        background: var(--canvas, #fafbfc);
    }
    .kg-toggle {
        display: flex;
        align-items: baseline;
        gap: 0.5em;
        width: 100%;
        background: none;
        border: none;
        padding: 0.3em 0;
        cursor: pointer;
        font: inherit;
        font-weight: 700;
        color: var(--fg, #202124);
        text-align: left;
    }
    .kg-toggle .chev { font-size: 0.9em; opacity: 0.7; width: 1em; }
    .kg-toggle-sub { font-weight: 400; font-size: 0.8em; opacity: 0.6; }
    .kg-toggle:hover { color: #1a73e8; }
    .kg-legend { display: flex; gap: 0.9em; font-size: 0.8em; opacity: 0.85; margin-top: 0.7em; flex-wrap: wrap; }
    .kg-legend span { display: inline-flex; align-items: center; gap: 0.3em; }
    .kg-legend i { width: 11px; height: 11px; border-radius: 50%; display: inline-block; }
    .kg-msg { padding: 2em; text-align: center; opacity: 0.7; }
    .kg-body { display: flex; gap: 1em; margin-top: 0.8em; flex-wrap: wrap; }
    .kg-svg { flex: 1 1 560px; min-width: 320px; height: auto; }
    .kg-col { font-size: 13px; font-weight: 700; fill: var(--fg-subtle, #5f6368); }
    .edge { stroke-linecap: round; }
    .edge.prereq { stroke: #9aa0a6; stroke-width: 1.6; }
    .edge.related { stroke: #c7b3e6; stroke-width: 1.2; stroke-dasharray: 3 4; opacity: 0.7; }
    .node { cursor: pointer; }
    .node:hover circle { filter: brightness(1.08); }
    .node.sel circle { stroke: #1a73e8; stroke-width: 3; }
    .kg-node-r { font-size: 11px; font-weight: 700; fill: #fff; }
    .kg-node-lbl { font-size: 10px; fill: var(--fg, #202124); }
    .kg-badge { font-size: 10px; font-weight: 700; fill: #fff; }
    .kg-side { flex: 1 1 220px; min-width: 200px; display: flex; flex-direction: column; gap: 0.8em; }
    .kg-panel {
        border: 1px solid var(--border, #e3e6e8);
        border-radius: 8px;
        padding: 0.7em 0.9em;
        background: var(--canvas, #fafbfc);
    }
    .kg-panel-title { font-weight: 700; margin-bottom: 0.4em; }
    .kg-panel ol { margin: 0; padding-left: 1.1em; }
    .kg-panel li { margin: 0.15em 0; display: flex; align-items: center; gap: 0.4em; }
    .dot { width: 9px; height: 9px; border-radius: 50%; display: inline-block; flex: none; }
    .linkish {
        background: none; border: none; padding: 0; color: #1a73e8;
        cursor: pointer; text-align: left; font: inherit;
    }
    .linkish:hover { text-decoration: underline; }
    .kg-hint { font-size: 0.75em; opacity: 0.65; margin-top: 0.4em; }
    .kg-panel table { width: 100%; font-size: 0.85em; border-collapse: collapse; }
    .kg-panel td { padding: 0.15em 0; }
    .kg-panel td:first-child { opacity: 0.7; padding-right: 0.6em; }
    .kg-study {
        margin-top: 0.6em; width: 100%; padding: 0.4em; border: none; border-radius: 6px;
        background: #1a73e8; color: #fff; cursor: pointer; font-weight: 600;
    }
    .kg-study:hover { background: #1663c7; }
    .kg-empty { opacity: 0.6; font-size: 0.85em; }
</style>
