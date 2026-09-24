# Phase 10 agent batch record (LF-80, LF-161, LF-162)

Date: 2026-09-23 to 2026-09-24
Linear: LF-80 (Phase 6 fast-follow), LF-161 (Phase 10a), LF-162 (Phase 10b), all children of or blockers for LF-53
Driver: `/flow-batch` orchestrator session. Each issue ran as its own `/flow-issue` agent: Claude
implemented, Claude specialists reviewed and Codex was the cross-model adversary.

This is the durable record of how the three changes were built, reviewed and merged, what was
deferred, and what the run taught us. The per-phase details live in `phase-6b.md`, `phase-10a.md`
and `phase-10b.md`.

## Outcome

| Merge | Issue | PR | Merge commit | Review lane |
|---|---|---|---|---|
| 1 | LF-80 | https://github.com/lukerf89/comlink/pull/15 | 09691f2 | lite + 1 fix round |
| 2 | LF-161 | https://github.com/lukerf89/comlink/pull/17 | d8faab5 | full + 3 fix rounds + rebase round |
| 3 | LF-162 | https://github.com/lukerf89/comlink/pull/18 | 7bd36ed | full + 1 fix round (4 confirming Codex passes) |

LF-80 and LF-161 ran in parallel, each confined to its own part of `src/cli.rs`: LF-80 to
`record_memo` and the doctor code, LF-161 to the `meet_*` functions. LF-80 merged first, and LF-161
was then rebased onto it. That rebase had no textual conflicts, but clippy failed on one unused
import. LF-162 started only after both had merged, and its brief carried their requirements (below).

Before every merge, the orchestrator independently ran `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, three consecutive `cargo test --all` runs, and
`cargo run -- doctor`, on the PR head in a separate worktree.
LF-162 also got a live stdio smoke test of `comlink mcp`, which checked four things:
- every stdout line is JSON-RPC and nothing reaches stderr;
- tool annotations are correct;
- `meeting_start` is refused while `allow_start=false`;
- a path-traversal resource URI is rejected.

## Review findings that changed the code

### LF-80
- A flaky reap test raced a pid file against the kill. It now takes the pid from `Child::id()`. The
  stress loop found a second flake, a hang-bound margin that was too tight.
- `doctor` ignored `COMLINK_RECORD_DEVICE` when ffmpeg was missing, and it mislabelled the device
  source.

### LF-161
- **Privacy (Codex high).** Detached finalize marked a session `stopped` before it deleted the
  unretained chunks. If the delete failed, raw audio stayed on disk with `retention.audio=false`,
  and nothing retried the cleanup. Fixed: chunks are deleted before `stopped` on both the sync and
  detached paths, a cleanup failure leaves the session `failed`, and rerunning finalize completes
  it.
- Later confirming rounds each found one more case where `privacy audit` would wrongly report
  clean: a corrupt `session.json`, a `chunks/` directory it couldn't search (EACCES swallowed by
  `is_dir`), and a moved data dir. The audit now has a conservative `meeting_audio` section that
  lists `scan_errors`.
- The finalizer child is reaped, so a long-lived parent (the MCP server) collects no zombie
  processes.
- A bare `meet export` right after `stop --detach` exported the previous meeting. A bare
  `meet status` showed a failed finalize as `none`.

### LF-162
- **The first Codex pass was hollow**: 95 seconds on a 6,074-line diff, with no findings. A
  re-run over the whole branch diff, given a per-area checklist, found:
  - HIGH: two concurrent `meeting_start` calls could both record. Fixed with `start.lock`.
  - MED: a config read-modify-write race could undo `mcp.allow_start=false`. Every config write
    now goes through a locked update.
  - MED: transcript reads followed symlinks and tampered paths in `session.json`. They now use
    `openat` with `O_NOFOLLOW` from the session's own dir.
  - MED: stop signalled only the recorder leader pid. It now signals the process group and each
    tracked member.
- Four confirming passes fixed a TOCTOU, a Linux-only `pgrep -g`, a possible signal to a reused
  pgid, and a surviving descendant, then approved.
- Out-of-plan hardening: meeting recorders run in their own process group and a thread reaps them.
  Otherwise the long-lived `comlink mcp` would accumulate zombie recorders, and a client that
  killed the server would also kill the recording.

## Known gaps (follow-up candidates)

From LF-161:
- A `failed` session with a valid JSON export but a missing `.md` is re-transcribed instead of
  being recovered from the export. No privacy impact.
- For an unreadable `chunks/` dir, the audit suggests `meet finalize`, which fails on the same dir.
  The permissions have to be fixed by hand.
- The N2 dedupe gives the wrong reason text for stray WAVs in the recorded chunks dir. It still
  reports `clean=false`.
- The early `chunks_processed` save in sync stop has no direct test.
- The finalize fast path returns a bare Io error, leaving the session `stopped`, when the chunks
  dir can't be inspected. The audit flags it.
- An ASR failure during sync stop leaves a preliminary `stopped`. The audit flags it and
  `finalize` recovers it.
- The busy-lock test fixture is load-sensitive. It flaked once in about 40 runs and has not
  reproduced since.
- The detached finalizer does not survive logout or reboot. Recover with `meet status` (stale) and
  then `meet finalize <id>`.

From LF-162:
- `transcribe_file` is not built (it was optional in the issue). There is no `outputSchema`, and the
  2024-11-05 MCP protocol version is not supported.
- A check-then-kill window remains between verifying the recorder leader and `kill -pgid`. It can't
  be closed on macOS without pidfd, and the pre-LF-162 code had the same window.
- The Claude reviewers' low findings are listed in the PR 18 body.

## Manual gates outstanding (at time of merge)

All three merged under the orchestrator's merge-on-green policy before the human manual tests ran:
- `phase-6b.md`: the stub-model `[warn]`, `doctor --probe-mic`, and `record` in a silent room.
- `phase-10a.md`: a real ~2 min `meet start`, then `meet status`, then `meet stop --detach`, with
  the terminal released immediately and the status going `transcribing`, then `stopped`.
- `phase-10b.md`:
  - register with Claude Code (`claude mcp add comlink -- <path>/comlink mcp`);
  - register with Claude Desktop, and check the mic-permission (TCC) guidance;
  - `meeting_start` is refused until `comlink config set mcp.allow_start true`;
  - the `privacy audit` MCP wording is understandable.

## Lessons for future agent runs

- **Check review effort against diff size.** A fast, empty adversary result on a large diff means
  no adversary ran. Have Codex review the whole branch diff (`git diff origin/main...HEAD`), not
  one commit (`--commit` once landed on an artifacts-only commit), and give it a checklist that
  requires file:line evidence per area. Here that turned "no findings" into 1 high and 3 mediums.
- **Privacy false-cleans come in families.** Each confirming round on the LF-161 audit found one
  more way to report clean while audio remained. When fixing one, test the siblings too:
  unreadable dirs, corrupt state, moved dirs, and error paths that return early.
- **Long-lived parents change process hygiene.** Code that was fine for a short-lived CLI
  (unreaped children, a shared process group) breaks under a persistent server. Any new
  long-running adapter needs a zero-zombie test.
- **Parallel issues in one file.** Confining each agent to a named region of `src/cli.rs` gave a
  conflict-free rebase. Still re-run clippy after the rebase: an import one side stopped using
  only showed up there.
- **E2E scripts rewrite committed artifacts.** Running the phase E2E scripts regenerates timestamped
  files under `docs/validation/artifacts/`. Revert unrelated phases' artifacts before committing.
- **Keep the main checkout on `main`.** Agents doing fix rounds checked out their branch in
  the main checkout. Use a worktree for branch work, and put the main checkout back on `main`
  after merge.
