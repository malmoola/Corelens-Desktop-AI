import { describe, expect, it } from 'vitest';

import { applyProgress, bySeverity, canProposeAgain, diagnoseArgs, explainPrompt, fraction, startRun, stepState, waitingProposals } from './diagnose';
import type { Change, DiagnosePlanStep, DiagnoseProgress, DiagnoseReport } from './ipc';

const ev = (p: Partial<DiagnoseProgress>): DiagnoseProgress => ({
  runId: 'd1',
  projectId: 'p1',
  kind: 'device',
  scope: '',
  devices: [],
  device: '',
  state: '',
  done: 0,
  total: 0,
  command: '',
  summary: [],
  error: '',
  ...p,
});

const step = (p: Partial<DiagnosePlanStep>): DiagnosePlanStep => ({
  seq: 1,
  cause: 'c',
  device: 'SW2',
  kind: 'change',
  title: 't',
  commands: ['interface Gi1/0/1', ' switchport trunk allowed vlan add 20'],
  verify: [],
  impact: '',
  rollback: '',
  risk: '',
  status: 'ready',
  note: '',
  proposalId: null,
  ...p,
});

const change = (id: string, status: Change['status'], createdMs = 1): Change => ({ id, status, createdMs }) as unknown as Change;

const report = (p: Partial<DiagnoseReport>): DiagnoseReport =>
  ({
    id: 'd1',
    scope: { kind: 'project' },
    depth: 'quick',
    symptom: null,
    startedMs: 0,
    finishedMs: 0,
    cancelled: false,
    summary: [],
    devices: [],
    causes: [],
    findings: [],
    passed: [],
    notChecked: [],
    plan: [],
    diff: null,
    path: null,
    playbooks: [],
    ...p,
  }) as DiagnoseReport;

describe('a run as its events arrive', () => {
  it('counts devices and commands, and ignores other runs', () => {
    let r = startRun('d1');
    r = applyProgress(r, ev({ kind: 'started', devices: ['SW1', 'SW2'], scope: 'the whole project' }));
    expect(r.order).toEqual(['SW1', 'SW2']);
    expect(fraction(r)).toBe(0);
    r = applyProgress(r, ev({ device: 'SW1', state: 'running', done: 5, total: 10, command: 'show vlan brief' }));
    expect(fraction(r)).toBeCloseTo(0.25);
    r = applyProgress(r, ev({ device: 'SW2', state: 'unreachable', error: 'no route' }));
    expect(fraction(r)).toBeCloseTo(0.75);
    expect(r.devices.SW2?.error).toBe('no route');
    const other = applyProgress(r, ev({ runId: 'd2', device: 'SW1', state: 'done' }));
    expect(other).toBe(r);
    r = applyProgress(r, ev({ kind: 'finished', summary: ['2 of 2'] }));
    expect(r.finished && r.summary[0] === '2 of 2').toBe(true);
    expect(applyProgress(startRun('d1'), ev({ kind: 'failed', error: 'vault locked' })).error).toBe('vault locked');
  });
});

describe('a plan step against the change record', () => {
  it('follows its proposal', () => {
    expect(stepState(step({ status: 'manual' }), {})).toBe('manual');
    expect(stepState(step({}), {})).toBe('ready');
    expect(stepState(step({ status: 'proposed', proposalId: 'c1' }), {})).toBe('pending');
    for (const [status, want] of [['pending', 'pending'], ['running', 'running'], ['done', 'done'], ['failed', 'failed'], ['denied', 'denied'], ['timedOut', 'denied'], ['withdrawn', 'denied']] as const) {
      expect(stepState(step({ status: 'proposed', proposalId: 'c1' }), { c1: change('c1', status) })).toBe(want);
    }
    expect(canProposeAgain('denied') && canProposeAgain('failed') && !canProposeAgain('pending') && !canProposeAgain('done')).toBe(true);
  });

  it('opens the oldest waiting proposal first', () => {
    const r = report({ plan: [step({ seq: 1, proposalId: 'b', status: 'proposed' }), step({ seq: 2, proposalId: 'a', status: 'proposed' }), step({ seq: 3, proposalId: 'z', status: 'proposed' })] });
    expect(waitingProposals(r, { a: change('a', 'pending', 1), b: change('b', 'pending', 2), z: change('z', 'done', 0) })).toEqual(['a', 'b']);
  });
});

describe('the buttons and the report', () => {
  it('starts each kind of run with its own arguments', () => {
    expect(diagnoseArgs.project()).toEqual({ scope: 'project' });
    expect(diagnoseArgs.link('SW1', 'SW2')).toEqual({ scope: 'devices', targets: ['SW1', 'SW2'] });
    expect(diagnoseArgs.link('SW1', 'SW1')).toEqual({ scope: 'devices', targets: ['SW1'] });
    expect(diagnoseArgs.path('SW1', '192.0.2.9', 443)).toEqual({ scope: 'path', src: 'SW1', dst: '192.0.2.9', port: 443 });
  });

  it('groups findings worst first and asks the chat about the top cause', () => {
    const f = (severity: 'info' | 'warning' | 'critical', device: string) => ({ id: device + severity, check: 'x', title: 't', layer: 'l2' as const, severity, device, says: '', fields: {}, evidence: [] });
    const r = report({ findings: [f('info', 'A'), f('critical', 'B'), f('critical', 'A')], causes: [{ id: 'c', title: 'VLAN 20 is not allowed', severity: 'critical', confidence: 0.9, why: '', findings: [], supporting: [], rulingOut: [] }] });
    const g = bySeverity(r);
    expect(g.map((x) => x.severity)).toEqual(['critical', 'info']);
    expect(g[0]?.findings.map((x) => x.device)).toEqual(['A', 'B']);
    expect(explainPrompt(r)).toContain('VLAN 20 is not allowed');
    expect(explainPrompt(r)).toContain('d1');
  });
});
