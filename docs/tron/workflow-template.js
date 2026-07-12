export const meta = {
  name: 'tron-plans-fg',
  description: 'Планы F+G: авторезолвер tron-solc и read-only fork-режим через /jsonrpc',
  phases: [
    { title: 'F1: resolver crate', detail: 'foundry-tron-solc: скачивание + sha256-пины', model: 'opus' },
    { title: 'F2: config integration', detail: 'ensure_solc + sandbox на авторезолв', model: 'opus' },
    { title: 'Review F', detail: 'Fable-ревью плана F', model: 'fable' },
    { title: 'G1: nonce shim', detail: 'tower-layer eth_getTransactionCount→0x0, включение fork', model: 'opus' },
    { title: 'G2: fork tests', detail: 'live mainnet USDT fork-тесты + доки', model: 'opus' },
    { title: 'Final Review', detail: 'Fable: планы F+G целиком', model: 'fable' },
  ],
}

const VERDICT = {
  type: 'object',
  properties: {
    passed: { type: 'boolean', description: 'true only if EVERY check passed' },
    issues: { type: 'array', items: { type: 'string' }, description: 'concrete actionable issues (file:line, wrong vs expected); empty if passed' },
    summary: { type: 'string', description: '2-4 sentences overall assessment' },
    domain_facts_checked: { type: 'array', items: { type: 'string' }, description: 'domain facts independently re-verified (release assets, checksums, live /jsonrpc probes, fork behavior)' },
  },
  required: ['passed', 'issues', 'summary', 'domain_facts_checked'],
  additionalProperties: false,
}

const CTX = `## Environment (this machine)
- Repo: /Users/andrey/vibe_projects/foundry-tron — Foundry fork with Tron support. Work ONLY inside it.
- Branch: tron-stage2 (checked out; carries completed plan E — faithful tron-revm). Commit locally. NEVER push, NEVER switch branches.
- Rust: export PATH="$HOME/.cargo/bin:$PATH" (stable 1.97, nightly rustfmt). cargo +nightly fmt. Long builds: Bash timeout 600000, re-run on timeout. forge/cast prebuilt (cast = package cast@1.7.2).
- THE PLANS (read yours FULLY first): docs/tron/plans/2026-07-12-tron-solc-resolver.md (plan F) and docs/tron/plans/2026-07-12-tron-fork-mode.md (plan G) — verified facts, seams, per-task steps. Scout reports: /private/tmp/claude-501/-Users-andrey-vibe-projects-foundry-tron/fd1be1e6-efb6-46bc-b20a-013c4fd694c5/scratchpad/scout-stage2/solc-resolver.md and fork-mode.md.
- java-tron sources via https://cdn.jsdelivr.net/gh/tronprotocol/java-tron@develop/<path> (raw.githubusercontent blocked). GitHub API via gh CLI works. tronprotocol.github.io/solc-bin list.json reachable.
- Live: mainnet /jsonrpc = https://api.trongrid.io/jsonrpc (read-only, no key needed, be moderate — rate limits). Nile /wallet = https://api.nileex.io (no /jsonrpc there). Key .env.tron-dev exists (git-ignored) but plans F/G need NO TRX spending. Network is flaky: curl -4, retries.
- Conventions: repo-root CLAUDE.md. Fork tests MUST contain "fork" in the name (repo rule).

## Hard rules (approved process — violations = verification failure)
- Never weaken asserts, never #[ignore], no tautology. Pins/vectors from real sources (live list.json, real releases, live RPC responses).
- Gated tests: TRON_SOLC_DOWNLOAD=1 for the real-download test, TRON_LIVE=1 for live fork tests — explicit eprintln skip when unset.
- No new external dependencies (reqwest/sha2/tower already in workspace — verify feature availability before assuming; follow foundry_compilers' RuntimeOrHandle::block_on pattern for blocking HTTP if needed).
- foundry-fork-db and other git-pinned deps are NOT patchable — layer on our side only.
- Non-tron networks untouched; their tests stay green.
- Conventional commits ending with:
Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`

function implPrompt(planTask, title, planFile, extra) {
  return `${CTX}

# Your task: implement ${planTask} (${title})

Read the ENTIRE plan file ${planFile} first, plus the referenced scout report. Execute your task step by step exactly as written; line anchors are recon-level — navigate by named items. Verify exact API shapes (alloy-transport layers, retry_layer style, foundry-compilers Solc::new_with_version, RuntimeOrHandle) against actual sources in ~/.cargo before use.
${extra}
Your final message is consumed by an orchestrator script. Return raw data (English): steps completed; test summaries; live evidence if applicable; commit hash(es); deviations and why; domain facts resolved on the spot.`
}

function verifyPrompt(planTask, title, planFile, report, extra) {
  return `${CTX}

# Your task: independently VERIFY ${planTask} (${title}) — adversarial; do NOT fix, do NOT commit

Implementer's report:
<implementer_report>
${report}
</implementer_report>

Do not trust the report — re-derive everything. This process has repeatedly caught real bugs and wrong domain facts at every stage. Be adversarial:
1. Re-read the plan (${planFile}); inspect the diff (git show/diff of task commits).
2. Re-run all offline tests/checks yourself (touched crates: cargo test, clippy --all-targets, fmt --check; cargo check --workspace; sandbox forge gate if touched — rebuild forge first when evm/config crates changed).
3. Re-verify domain facts from FIRST SOURCES: live list.json checksums, gh api release assets, live /jsonrpc probes, java-tron sources via jsdelivr — as applicable to this task.
4. Gated tests: run the gate yourself once if feasible (TRON_SOLC_DOWNLOAD is heavy on this slow network — judge by size; TRON_LIVE fork tests are read-only and cheap — run them).
5. Non-tautology, no weakened asserts, no #[ignore], explicit skip lines.
6. Regression: non-tron paths untouched (run a relevant non-tron slice).
7. Style/commit hygiene per CLAUDE.md.
${extra}
passed=true ONLY if every check passes. Each failure = separate concrete issue.`
}

function fixPrompt(label, issues) {
  return `${CTX}

# Your task: FIX rejected verification issues (${label})

Fix ALL:
${issues.map((s, i) => `${i + 1}. ${s}`).join('\n')}

Root causes only, never weaken tests. Re-run relevant checks until green. Amend the task commit unless a separate fix commit is cleaner. Return: changes per issue, check results, final commit hash(es).`
}

async function verifiedTask(phaseName, label, planTask, title, planFile, implExtra, verifyExtra) {
  log(`${label}: Opus implementer starting`)
  let report = await agent(implPrompt(planTask, title, planFile, implExtra), { model: 'opus', phase: phaseName, label: `${label}:impl` })
  if (report === null) throw new Error(`${label}: implementer died`)
  let lastVerdict = null
  for (let round = 1; round <= 4; round++) {
    const v = await agent(verifyPrompt(planTask, title, planFile, report, verifyExtra), { model: 'opus', phase: phaseName, label: `${label}:verify-r${round}`, schema: VERDICT })
    if (v === null) throw new Error(`${label}: verifier died r${round}`)
    lastVerdict = v
    if (v.passed) { log(`${label}: verification PASSED r${round} — ${v.summary}`); return { report, verdict: v, ok: true } }
    log(`${label}: r${round} rejected: ${v.issues.join(' | ').slice(0, 300)}`)
    if (round === 4) break
    const fix = await agent(fixPrompt(label, v.issues), { model: 'opus', phase: phaseName, label: `${label}:fix-r${round}` })
    if (fix === null) throw new Error(`${label}: fixer died r${round}`)
    report = `${report}\n\n--- FIX ROUND ${round} ---\n${fix}`
  }
  return { report, verdict: lastVerdict, ok: false }
}

function fableReviewPrompt(scope, taskSummaries, round, isFinal) {
  return `${CTX}

# ${isFinal ? 'FINAL architecture review of plans F+G' : `Interim architecture review (${scope})`} — round ${round}; senior reviewer

Verifier summaries:
${taskSummaries}

Deep review (approved Fable checkpoint):
1. Read both plan files, the diff since commit 87abac2cf (plans doc), scout reports as needed.
2. Correctness & requirements: ${isFinal ? 'resolver downloads/pins/caches correctly and only on the tron path; fork works read-only over /jsonrpc with the nonce shim; both match their plans’ verified facts.' : 'the resolver crate + config seam match plan F’s facts (pins from real list.json, no svm/verify_checksum on tron binaries, offline honored, sandbox portable).'}
3. Architecture: layering clean (config -> tron-solc crate dep direction, no cycles; tower shim strictly on tron path; no git-pinned deps patched); non-tron behavior byte-identical.
4. Tests: real pins/vectors, gates with skips, fork tests named *fork*, no weakening.
5. Run yourself: cargo test on touched crates; cargo check --workspace; sandbox gate; ${isFinal ? 'the live fork test suite (TRON_LIVE=1, read-only mainnet) once' : 'sandbox 5/5 via auto-resolve (no absolute solc path left)'}.
6. Emergent findings recorded (STATUS at final).
7. Commit hygiene; nothing pushed.

passed=true ONLY if you would ${isFinal ? 'merge into tron-dev-continue as-is' : 'let plan G build on this as-is'}. Non-blocking notes in summary (Russian OK).`
}

async function fableGate(scope, phaseName, taskSummaries, isFinal) {
  let last = null
  for (let round = 1; round <= 4; round++) {
    const v = await agent(fableReviewPrompt(scope, taskSummaries, round, isFinal), { model: 'fable', phase: phaseName, label: `${scope}:review-r${round}`, schema: VERDICT })
    if (v === null) throw new Error(`${scope}: reviewer died r${round}`)
    last = v
    if (v.passed) { log(`${scope}: Fable review PASSED r${round}`); return { verdict: v, ok: true } }
    log(`${scope}: r${round} rejected: ${v.issues.join(' | ').slice(0, 300)}`)
    if (round === 4) break
    const fix = await agent(fixPrompt(scope, v.issues), { model: 'opus', phase: phaseName, label: `${scope}:fix-r${round}` })
    if (fix === null) throw new Error(`${scope}: fixer died r${round}`)
  }
  return { verdict: last, ok: false }
}

const PLAN_F = 'docs/tron/plans/2026-07-12-tron-solc-resolver.md'
const PLAN_G = 'docs/tron/plans/2026-07-12-tron-fork-mode.md'

phase('F1: resolver crate')
const f1 = await verifiedTask('F1: resolver crate', 'f1', 'Task F1', 'foundry-tron-solc resolver crate', PLAN_F,
  `Key specifics: pins MUST come from the live tronprotocol.github.io/solc-bin/{platform}/list.json (fetch now, embed as const with source comment + date; at least 0.8.25/26/27 for all three platforms). The 0.8.27 macos pin must equal 9e369b442b3a835320cf1450a21ce5e8acf0d319a50d10a10a2001aac7ce17aa (cross-check vs the actual binary at ~/.foundry-tron/solc/tron-solc-0.8.27). Atomic write (tmp+rename), chmod 755, bounded retries. The download-gated test may pick the smallest platform asset of one version to keep it feasible on this slow network.`,
  `Extra checks: re-fetch list.json yourself and diff every embedded pin against it; verify the cache-hit path never touches the network (e.g. by pointing at an unroutable env or reading the code path carefully + a test with a mock/unreachable URL if the design allows); confirm error on corrupted cache actually triggers (flip a byte in a temp copy); confirm linux-arm gives the clean error.`)
if (!f1.ok) return { plan: 'F+G', ok: false, failedAt: 'F1', verdict: f1.verdict }

phase('F2: config integration')
const f2 = await verifiedTask('F2: config integration', 'f2', 'Task F2', 'ensure_solc integration + portable sandbox', PLAN_F,
  `Key specifics: the tron branch in ensure_solc must honor self.offline and must NOT call Solc::verify_checksum or svm paths; return via Solc::new_with_version. Sandbox foundry.toml drops the absolute path — the machine cache at ~/.foundry-tron/solc/tron-solc-0.8.27 must satisfy resolution (verify its filename matches your crate's binary_path convention BEFORE relying on it; if the convention differs, migrate carefully). Do not break non-tron config tests. Where a test arm can't be honestly tested offline (0.8.26 resolution without its binary), gate it — never fake pins.`,
  `Extra checks: rm -rf sandbox out/cache and run forge build+test with a rebuilt forge — 5/5 through auto-resolve; grep the repo that no machine-absolute tron-solc path remains outside docs/history; run the full foundry-config suite; confirm offline=true + empty cache errors cleanly (temporarily move the cache aside for the check, then restore it).`)
if (!f2.ok) return { plan: 'F+G', ok: false, failedAt: 'F2', f1: f1.verdict, verdict: f2.verdict }

phase('Review F')
const rf = await fableGate('review-f', 'Review F', `F1: ${f1.verdict.summary}\nF2: ${f2.verdict.summary}`, false)
if (!rf.ok) return { plan: 'F+G', ok: false, failedAt: 'review-F', verdict: rf.verdict }

phase('G1: nonce shim')
const g1 = await verifiedTask('G1: nonce shim', 'g1', 'Task G1', 'tower nonce shim + fork enablement for tron', PLAN_G,
  `Key specifics: study how retry_layer is written in crates/common/src/provider/mod.rs and mirror its style for the shim layer (intercept eth_getTransactionCount -> "0x0", pass everything else through). Wire strictly on the tron path (NetworkConfigs::is_tron() at the fork-provider construction points). Add the early friendly diagnostic for a non-/jsonrpc fork_url on tron. Keep the stage-1 script-side fork rejection intact. Note interim review: ${rf.verdict.summary.slice(0, 200)}`,
  `Extra checks: unit-test the layer yourself with a mock transport confirming no inner call happens for the shimmed method; live-probe once that eth_getTransactionCount on api.trongrid.io/jsonrpc really returns -32601 (the fact the shim exists for); verify non-tron providers get NO shim (read the wiring + a test if present); run an Ethereum fork smoke if one exists offline (or the relevant unit suite) to prove no regression.`)
if (!g1.ok) return { plan: 'F+G', ok: false, failedAt: 'G1', verdict: g1.verdict }

phase('G2: fork tests')
const g2 = await verifiedTask('G2: fork tests', 'g2', 'Task G2', 'live mainnet fork coverage + docs', PLAN_G,
  `Key specifics: USDT mainnet contract (base58 TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t, hex a614f803b6fd780986a42c78ec9c7f77e6ded13c): assert name()=="Tether USD", decimals()==6, totalSupply()>0, balanceOf(rich addr)>0, extcodesize>0, a concrete storage slot read. Test names contain "fork". Gate TRON_LIVE=1 (read-only, no TRX spent). Check vm.createSelectFork flows through the shimmed provider too. STATUS.md and spec updated (plans F+G done, mainnet-only live channel note).`,
  `Extra checks: run the live fork suite yourself (TRON_LIVE=1); verify assertions against direct /jsonrpc eth_call probes (name/decimals independently); confirm offline suites untouched and workspace check clean; confirm script fork rejection message still present and accurate; check STATUS.md preserves all prior findings.`)
if (!g2.ok) return { plan: 'F+G', ok: false, failedAt: 'G2', g1: g1.verdict, verdict: g2.verdict }

phase('Final Review')
const fin = await fableGate('final', 'Final Review',
  `F1: ${f1.verdict.summary}\nF2: ${f2.verdict.summary}\nG1: ${g1.verdict.summary}\nG2: ${g2.verdict.summary}`, true)
if (!fin.ok) return { plan: 'F+G', ok: false, failedAt: 'final-review', verdict: fin.verdict }

return {
  plan: 'F+G', ok: true,
  f1: f1.verdict.summary, f2: f2.verdict.summary, g1: g1.verdict.summary, g2: g2.verdict.summary,
  reviewF: rf.verdict.summary, finalReview: fin.verdict.summary,
  factsChecked: [...f1.verdict.domain_facts_checked, ...f2.verdict.domain_facts_checked, ...g1.verdict.domain_facts_checked, ...g2.verdict.domain_facts_checked, ...fin.verdict.domain_facts_checked],
}