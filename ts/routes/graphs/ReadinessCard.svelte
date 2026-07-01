<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

MCAT readiness, rendered inside the Statistics page. Calls the in-engine
ComputeReadiness RPC (same one the phone uses) and shows the three separate
scores + ranges + give-up rule + the global next-best-action list.
-->
<script lang="ts">
    import { computeReadiness, getConfigJson } from "@generated/backend";
    import type { ReadinessResponse } from "@generated/anki/stats_pb";
    import { onMount } from "svelte";

    const SECTION_NAMES: Record<string, string> = {
        BB: "Biological & Biochemical",
        CP: "Chemical & Physical",
        PS: "Psych / Social",
        CARS: "Critical Analysis & Reasoning",
    };

    // AAMC outline weights (section -> {topic_id: within-section weight}).
    const OUTLINE: Record<string, Record<string, number>> = {
        BB: {
            "mcat::bb::amino_acids_proteins": 0.16, "mcat::bb::enzymes": 0.14,
            "mcat::bb::nucleic_acids": 0.12, "mcat::bb::metabolism": 0.14,
            "mcat::bb::cell_biology": 0.12, "mcat::bb::microbiology": 0.06,
            "mcat::bb::genetics": 0.1, "mcat::bb::organ_systems": 0.16,
        },
        CP: {
            "mcat::cp::atomic_structure": 0.08, "mcat::cp::bonding": 0.08,
            "mcat::cp::thermodynamics": 0.12, "mcat::cp::kinetics": 0.12,
            "mcat::cp::acids_bases": 0.12, "mcat::cp::electrochemistry": 0.08,
            "mcat::cp::mechanics": 0.12, "mcat::cp::electrostatics_circuits": 0.1,
            "mcat::cp::optics_sound": 0.08, "mcat::cp::organic_chemistry": 0.1,
        },
        PS: {
            "mcat::ps::sensation_perception": 0.12, "mcat::ps::learning_memory": 0.14,
            "mcat::ps::cognition_language": 0.12, "mcat::ps::motivation_emotion": 0.12,
            "mcat::ps::identity_personality": 0.12, "mcat::ps::social_processes": 0.14,
            "mcat::ps::social_structure": 0.12, "mcat::ps::demographics_inequality": 0.12,
        },
        CARS: {
            "mcat::cars::foundations_comprehension": 0.3,
            "mcat::cars::reasoning_within_text": 0.3,
            "mcat::cars::reasoning_beyond_text": 0.4,
        },
    };

    const ICONS: Record<string, string> = {
        cover: "\u{1F9ED}",
        review: "\u{1F501}",
        practice_check: "\u{1F4DD}",
    };

    let resp: ReadinessResponse | null = null;
    let errored = false;

    onMount(async () => {
        try {
            resp = await load();
        } catch (e) {
            console.error("readiness failed", e);
            errored = true;
        }
    });

    async function readPerf(): Promise<Record<string, { correct: number; total: number }>> {
        try {
            const res = await getConfigJson({ val: "mcat_perf" });
            const txt = new TextDecoder().decode(res.json);
            return JSON.parse(txt);
        } catch {
            return {};
        }
    }

    async function load(): Promise<ReadinessResponse> {
        const perf = await readPerf();
        const sections = Object.entries(OUTLINE).map(([section, topicWeights]) => {
            const rec = perf[section] ?? { correct: 0, total: 0 };
            const total = rec.total ?? 0;
            const performance = total ? rec.correct / total : 0;
            const thetaSe = total ? 1.5 / Math.sqrt(total + 1) : 9.9;
            return {
                section,
                topicPrefix: `mcat::${section.toLowerCase()}`,
                topicWeights,
                performance,
                thetaSe,
                alphaSpace: 1,
                alphaInter: 1.3,
                alphaTest: 1.5,
            };
        });
        return await computeReadiness({
            search: "",
            masteredThreshold: 0.7,
            sections,
            params: {
                b0: -1.0, bM: 1.3, bP: 2.2, bC: 0.5,
                minGradedReviews: 200, minCoverage: 0.5, maxIrtSe: 0.5,
            },
        });
    }

    function shortTopic(id: string): string {
        return id ? id.split("::").pop()!.replace(/_/g, " ") : "\u2014";
    }
</script>

{#if resp}
    <div class="readiness">
        {#if resp.total?.abstained}
            <div class="hero withheld">
                <div class="big">Projected MCAT: &mdash; withheld &mdash;</div>
                <div class="sub">Not enough evidence in: <b>{resp.total.missing.join(", ")}</b></div>
                <div class="why">
                    Intentional: the app won't guess until each section has 200+ graded
                    reviews and 50%+ coverage. Keep reviewing &mdash; sections unlock as
                    evidence accumulates.
                </div>
            </div>
        {:else if resp.total}
            <div class="hero ok">
                <div class="big">Projected MCAT: {Math.round(resp.total.value)}</div>
                <div class="sub">
                    Likely range {Math.round(resp.total.rangeLo)}&ndash;{Math.round(resp.total.rangeHi)}
                    &middot; confidence {resp.total.confidence}
                </div>
            </div>
        {/if}

        {#if resp.nextActions.length}
            <div class="panel">
                <div class="panel-title">Do this next (highest points at stake)</div>
                <ol>
                    {#each resp.nextActions as a}
                        <li>
                            <span class="atype">{ICONS[a.actionType] ?? "\u2022"} {a.actionType.replace("_", " ")}</span>
                            <b>{a.section} &middot; {a.topicId ? shortTopic(a.topicId) : "section check"}</b>
                            <div class="areason">{a.reason}</div>
                        </li>
                    {/each}
                </ol>
            </div>
        {/if}

        <div class="cards">
            {#each resp.sections as s}
                <div class="card">
                    <div class="sec">{s.section} &middot; {SECTION_NAMES[s.section] ?? ""}</div>
                    <div class="grid">
                        <div>
                            <div class="lbl">Memory</div>
                            <div class="val">{s.memory ? s.memory.value.toFixed(2) : "\u2014"}</div>
                            <div class="ev">{s.memory?.gradedReviews ?? 0} reviews &middot; {Math.round(s.memory?.coveragePct ?? 0)}% covered</div>
                        </div>
                        <div>
                            <div class="lbl">Performance</div>
                            <div class="val">{s.performance ? s.performance.value.toFixed(2) : "\u2014"}</div>
                            <div class="ev">transfer (IRT)</div>
                        </div>
                        <div>
                            <div class="lbl">Readiness</div>
                            {#if s.readiness?.abstained}
                                <div class="val wthld">withheld</div>
                                <div class="ev">need {s.readiness.missing.join(", ")}</div>
                            {:else if s.readiness}
                                <div class="val">{Math.round(s.readiness.value)}</div>
                                <div class="ev">[{Math.round(s.readiness.rangeLo)}&ndash;{Math.round(s.readiness.rangeHi)}] &middot; {s.readiness.confidence}</div>
                            {/if}
                        </div>
                    </div>
                </div>
            {/each}
        </div>

        <div class="caveat">
            Memory is live FSRS from your reviews; Performance is from your in-app question
            checks; the section&rarr;score map is evidence-based but not yet field-calibrated.
        </div>
    </div>
{:else if errored}
    <div class="readiness"><div class="caveat">MCAT readiness unavailable (no mcat:: tagged cards yet).</div></div>
{/if}

<style lang="scss">
    .readiness {
        width: calc(100vw - 3em);
        margin: 1em auto 0;
        max-width: 60em;
    }
    .hero {
        border-radius: 14px;
        padding: 18px;
        margin-bottom: 14px;
        border: 1px solid var(--border, rgba(128, 128, 128, 0.3));
    }
    .hero.ok { background: rgba(0, 160, 90, 0.12); }
    .hero.withheld { background: rgba(200, 120, 0, 0.12); }
    .big { font-size: 24px; font-weight: 700; }
    .sub { opacity: 0.8; margin-top: 4px; }
    .why { margin-top: 8px; font-size: 13px; opacity: 0.85; line-height: 1.5; }
    .panel {
        border: 1px solid rgba(80, 140, 255, 0.4);
        background: rgba(80, 140, 255, 0.08);
        border-radius: 12px; padding: 14px; margin-bottom: 14px;
    }
    .panel-title { font-weight: 700; margin-bottom: 8px; }
    .panel ol { margin: 0; padding-left: 20px; }
    .panel li { margin-bottom: 8px; }
    .atype { font-size: 11px; text-transform: uppercase; letter-spacing: 0.03em; opacity: 0.7; margin-right: 6px; }
    .areason { font-size: 12px; opacity: 0.75; line-height: 1.4; margin-top: 2px; }
    .cards { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
    @media only screen and (max-width: 900px) { .cards { grid-template-columns: 1fr; } }
    .card { border: 1px solid rgba(128, 128, 128, 0.3); border-radius: 12px; padding: 14px; }
    .sec { font-weight: 600; margin-bottom: 10px; }
    .grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; }
    .lbl { font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; opacity: 0.6; }
    .val { font-size: 20px; font-weight: 700; margin-top: 2px; }
    .val.wthld { color: #c87800; font-size: 16px; }
    .ev { font-size: 11px; opacity: 0.65; margin-top: 2px; }
    .caveat { margin-top: 10px; font-size: 12px; opacity: 0.7; line-height: 1.5; }
</style>
