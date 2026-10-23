# Test Audit Workflow

A suite-wide pass that measures the existing tests against `AGENTS.md` -> Testing, reports what costs runtime or maintenance without guarding a contract, and, once the user approves a plan, removes, merges, rewrites or marks those tests without losing a guarded contract. Run it when the user asks, over the whole suite or a scope the user names; the adoption report or a slow push may suggest it.

The rules are `AGENTS.md` -> Testing, the testing guide named there (what to assert, how to wait, what the fixtures isolate) and `.vorch/PROJECT.md` -> Testing (layout, markers, commands). The root maps state each domain's owners and contracts, which are what the tests must guard. The audit applies these rules and sets no new ones.

A large suite is too much for one context: subagents survey groups of tests and write compact ledgers, and the auditing agent compares them. Where your environment offers no subagents, process the groups one after another and write each ledger to a scratch file before you start the next group. Work in a worktree (`AGENTS.md` -> Git) and commit per domain, so every step can be reverted on its own.

## Dimensions

1. **Change records:** a test that pins one past change rather than a contract: it asserts an implementation detail, a private call sequence, an exact log or help wording, or a value that no caller relies on. Cure: delete it, or rewrite it as an assertion of the contract the change served.
2. **Wrong level:** a test of an internal helper whose behavior is reachable through the owner's public interface. Keep it when the helper carries its own contract that is impractical to reach that way. Cure: move the assertion to the public interface, then delete the helper test.
3. **Duplicates:** two or more tests asserting the same behavior, often one per bug fix. Cure: one test, parameterized only over genuinely different cases.
4. **Slow:** a test that needs more than about a second. Causes, in the order to check: real waits or sleeps, real network or subprocesses, durable I/O (disk syncs, fresh databases), expensive setup rebuilt per test, load or scale variants. Cure, in that order: controlled clock or explicit synchronization, in-memory fake, the suite's sync-skipping and `durable` marker, setup built once per worker, `stress` marker. Never a longer timeout.
5. **Flaky or order-dependent:** a test that failed and then passed without a change (push and CI reports, rerun lists), that depends on test order, wall-clock time, randomness, the real home directory, shared temp paths or tasks left running by earlier tests. Cure: fix the isolation or timing; delete it only when what it guards is asserted elsewhere.
6. **Brittle assertions:** exact text for editable prose, error messages or help, snapshot files nobody reviews, assertions on ordering the contract does not promise. Cure: assert types, codes, structured fields, roles or security invariants instead.
7. **Misplaced files:** test files named or placed after source files instead of owners and behaviors (`.vorch/PROJECT.md` -> Testing), or test modules beyond about 1,000 lines. Cure: regroup by owner and behavior.
8. **Contract gaps:** a cross-task contract in a root map (Interfaces, Data Model, invariants) that no test asserts. Report it; write a test only when the plan item says so.

## Judging a candidate

- A finding needs evidence: the test's node id, what it asserts, and either the contract it guards (map section or public name) or why it guards none. "Looks redundant" is a candidate, not a finding.
- A test may only be deleted or merged when every contract it guards is asserted by a test that stays. Name that test.
- Severity: 🔴 costs every run or every change in the area now (a slow test in a hot path, a flaky test, a duplicate cluster that every change must update), 🟡 real, schedule it, 🔵 opportunistic. Contract gaps carry no severity; they form their own list.

Report only findings that survive step 5 of the procedure.

## Procedure

1. **Measure.** Run the complete suite once with per-test durations (for pytest: `python -m pytest --durations=0 -q`; other runners: their reporter for per-test times) and record the total runtime, the number of tests per directory and the 50 slowest tests. Collect the flaky tests that recent push and CI reports name. This is the only full run before step 7; afterwards run only the tests a change affects.
2. **Partition.** Group the test directories along the Domain Maps index, at most about 15,000 lines of test code per group.
3. **Gather ledgers.** Give each group to a subagent. It reads this workflow, the testing rules and guide, and the group's root maps; it inspects the group's tests for all dimensions and confirms each candidate against source and the other tests. It changes no files and returns a ledger of at most 4,000 characters:
   - per domain, the contracts its tests guard, one line each, with the tests that guard them;
   - each finding: dimension, node id, evidence, the test that keeps guarding the contract (for deletions and merges), severity, cure;
   - each contract gap: map section and contract;
   - leads into other groups: a suspected duplicate whose other side lives elsewhere.
4. **Compare.** Resolve each lead by reading both sides, merge findings reported from both ends, and drop leads that do not hold.
5. **Challenge.** Give each group's deletions and merges to a fresh subagent that tries to show a contract that would lose its last test. Drop what it shows.
6. **Present the report and plan** (Output below) and wait for the user's approval of all, some or none of the items. No file has changed up to here.
7. **Implement per domain.** For each approved domain, one subagent (in its own worktree when several run in parallel, with disjoint test files):
   - before changing anything, measures the coverage of the domain's owners with the domain's tests (for pytest: `python -m pytest <tests> --cov=<source> --cov-branch --cov-report=term-missing`) and keeps the report;
   - applies the plan items: deletes, merges, rewrites, moves, marks; changes no application code unless a plan item says so;
   - measures coverage again: a line or branch the domain's tests covered before and not after is a loss, unless the plan item states the code is unreachable or dead. Restore every loss with an assertion through the public interface;
   - runs the domain's tests, commits, and returns a change report: per plan item, what was deleted, merged, moved or marked, the test that now guards each affected contract, coverage before and after, the runtime before and after.
8. **Verify with fresh subagents** that did not write the changes, one per domain: check every deleted or merged test against the change report and the diff (its contract is asserted by a named test that exists), confirm that no application code changed without a plan item, and that markers match the testing rules. The writing subagent fixes what it finds; verify again until nothing is left.
9. **Merge** each verified worktree with `python scripts/worktree.py merge <name>`, one after another. Then run the complete suite once more and compare runtime and test count with step 1.

## Output

The report and plan in step 6:

```markdown
## Test Audit: <scope>

### Summary
[Suite runtime, test count, the share of runtime in the slowest 50 tests, and the few things that matter most.]

### Findings
T1. **[severity] <dimension>, `node id`:** [what it asserts, why it costs more than it guards, the evidence]. [The cure; for a deletion or merge, the test that keeps guarding the contract.]

### Contract gaps
G1. `<map>.md` -> <section>: <contract> - no test asserts it.

### Plan
[The implementation groups per domain: which T items each takes, its test files, the expected runtime change, and the merge order.]
```

The final report after step 9: per plan item, whether it was implemented and, if not, why; runtime and test count before and after; coverage losses found and how they were restored; what verification found. Approved items that were not implemented go to `.vorch/FLAGGED.md`.
