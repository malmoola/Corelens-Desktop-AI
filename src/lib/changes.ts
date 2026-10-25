/**
 * The approval dialog's rules, out of the component: which devices can be
 * approved as the engineer has filled the dialog in, how a status reads,
 * and which proposal to show first.
 *
 * Rust checks the typed names again before it approves anything
 * (`check_confirmations`); this is what keeps the button honest.
 */
import type { Change, ChangeTarget, ChangeTargetStatus } from './ipc';

/** Whether this device's lines need its name typed to approve. */
export function needsTypedName(t: ChangeTarget): boolean {
  return t.assessment.hardBlocks.length > 0;
}

/** The seqs that may be approved as the dialog stands: ticked, and typed
 *  where the device needs it. */
export function approvable(change: Change, ticked: Set<number>, typed: Record<number, string>): number[] {
  return change.targets.filter((t) => ticked.has(t.seq) && (!needsTypedName(t) || (typed[t.seq] ?? '').trim() === t.device)).map((t) => t.seq);
}

/** What the engineer still has to do before "Approve" does anything. */
export function missing(change: Change, ticked: Set<number>, typed: Record<number, string>): string[] {
  return change.targets.filter((t) => ticked.has(t.seq) && needsTypedName(t) && (typed[t.seq] ?? '').trim() !== t.device).map((t) => t.device);
}

/** The args `change_decide` takes: the approved seqs and, in the same
 *  order, the name typed for each. */
export function decision(approve: number[], typed: Record<number, string>): { approve: number[]; typed: string[] } {
  return { approve, typed: approve.map((seq) => (typed[seq] ?? '').trim()) };
}

/** The proposal waiting longest, which the dialog shows first. A
 *  diagnosis's fix plan does not take the dialog by itself — the engineer
 *  is reading its report and opens the plan with "Review fix plan" — so
 *  its steps are left out unless `diagnose` is asked for. */
export function nextPending(changes: Record<string, Change>, diagnose = false): Change | null {
  return (
    Object.values(changes)
      .filter((c) => c.status === 'pending' && (c.source === 'diagnose') === diagnose)
      .sort((a, b) => a.createdMs - b.createdMs)[0] ?? null
  );
}

/** Seconds left before a proposal counts as denied. */
export function secondsLeft(change: Change, nowMs: number): number {
  return Math.max(0, Math.round((change.deadlineMs - nowMs) / 1000));
}

/** Whether a device of a change can be rolled back from the window. */
export function canRollBack(status: ChangeTargetStatus): boolean {
  return status === 'verifyFailed' || status === 'verified' || status === 'deviceRollingBack' || status === 'rollbackFailed';
}

/** The tone a status is drawn in. */
export function tone(status: ChangeTargetStatus): 'ok' | 'warn' | 'bad' | 'quiet' {
  switch (status) {
    case 'verified':
    case 'rolledBack':
    case 'saved':
      return 'ok';
    case 'verifyFailed':
    case 'deviceRollingBack':
    case 'running':
      return 'warn';
    case 'pushFailed':
    case 'rollbackFailed':
    case 'saveFailed':
      return 'bad';
    default:
      return 'quiet';
  }
}
