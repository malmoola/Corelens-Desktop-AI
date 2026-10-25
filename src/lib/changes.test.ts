import { describe, expect, it } from 'vitest';

import { approvable, canRollBack, decision, missing, nextPending, secondsLeft, tone } from './changes';
import type { Change, ChangeTarget } from './ipc';

const target = (seq: number, device: string, hard: boolean): ChangeTarget => ({
  seq,
  device,
  address: '192.0.2.10',
  platform: 'ciscoIos',
  platformName: 'Cisco IOS / IOS-XE',
  commands: ['interface Vlan10', ' shutdown'],
  verifyCommands: [],
  assessment: { class: hard ? 'high' : 'low', hardBlocks: hard ? [{ command: 'shutdown', risk: 'cuts it off' }] : [], summary: '' },
  predicted: [],
  predictedIsModel: true,
  rollbackPlan: '',
  status: 'pending',
  message: '',
  snapshot: '',
  actual: [],
  push: [],
  verification: [],
  rollback: [],
  confirmByMs: null,
  updatedMs: 0,
});

const change = (id: string, createdMs: number, targets: ChangeTarget[], status: Change['status'] = 'pending'): Change => ({
  id,
  projectId: 'p1',
  kind: 'change',
  source: 'chat',
  proposer: 'Corelens chat',
  reason: 'r',
  createdMs,
  deadlineMs: createdMs + 600_000,
  status,
  decidedMs: null,
  decidedBy: '',
  note: '',
  targets,
});

describe('the approval dialog', () => {
  it('approves a hard-blocked device only with its exact name typed', () => {
    const c = change('a', 0, [target(0, 'SW1', true), target(1, 'SW2', false)]);
    const ticked = new Set([0, 1]);
    expect(approvable(c, ticked, {})).toEqual([1]);
    expect(missing(c, ticked, {})).toEqual(['SW1']);
    expect(approvable(c, ticked, { 0: 'sw1' })).toEqual([1]);
    expect(approvable(c, ticked, { 0: ' SW1 ' })).toEqual([0, 1]);
    expect(decision([0, 1], { 0: ' SW1 ' })).toEqual({ approve: [0, 1], typed: ['SW1', ''] });
    expect(approvable(c, new Set([1]), {})).toEqual([1]);
  });

  it('shows the proposal waiting longest first and counts down to its deadline', () => {
    const older = change('o', 100, [target(0, 'SW1', false)]);
    const newer = change('n', 200, [target(0, 'SW1', false)]);
    const done = change('d', 50, [target(0, 'SW1', false)], 'done');
    expect(nextPending({ n: newer, o: older, d: done })?.id).toBe('o');
    expect(nextPending({ d: done })).toBeNull();
    // A diagnosis's plan waits for "Review fix plan" rather than taking the dialog.
    const plan: Change = { ...change('p', 10, [target(0, 'SW1', false)]), source: 'diagnose' };
    expect(nextPending({ p: plan, n: newer })?.id).toBe('n');
    expect(nextPending({ p: plan })).toBeNull();
    expect(nextPending({ p: plan, n: newer }, true)?.id).toBe('p');
    expect(secondsLeft(older, 100 + 590_000)).toBe(10);
    expect(secondsLeft(older, 10_000_000)).toBe(0);
  });

  it('offers rollback only where something was pushed, and tones each status', () => {
    expect(canRollBack('verifyFailed') && canRollBack('deviceRollingBack') && canRollBack('verified')).toBe(true);
    expect(canRollBack('denied') || canRollBack('pushFailed') || canRollBack('rolledBack')).toBe(false);
    expect([tone('verified'), tone('verifyFailed'), tone('rollbackFailed'), tone('denied')]).toEqual(['ok', 'warn', 'bad', 'quiet']);
  });
});
