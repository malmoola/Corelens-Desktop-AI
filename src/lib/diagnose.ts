/**
 * Diagnose network, the page's side: what the panel shows of a run as its
 * events arrive, how a report's plan steps stand against the change
 * record, and the arguments each Diagnose button starts a run with.
 *
 * Rust runs the sweep, judges it and proposes the fixes; this only keeps
 * count and words it.
 */
import type { Change, DiagnoseArgs, DiagnosePlanStep, DiagnoseProgress, DiagnoseReport, DiagnoseSeverity } from './ipc';

export type DeviceState = 'waiting' | 'running' | 'done' | 'unreachable' | 'stopped';

/** A run as the panel draws it while it goes on. */
export interface LiveRun {
  runId: string;
  scope: string;
  devices: Record<string, { state: DeviceState; done: number; total: number; command: string; error: string }>;
  order: string[];
  finished: boolean;
  error: string | null;
  summary: string[];
}

export function startRun(runId: string): LiveRun {
  return { runId, scope: '', devices: {}, order: [], finished: false, error: null, summary: [] };
}

/** The run after one event; events of other runs leave it as it was. */
export function applyProgress(run: LiveRun, p: DiagnoseProgress): LiveRun {
  if (p.runId !== run.runId) return run;
  switch (p.kind) {
    case 'started': {
      const devices: LiveRun['devices'] = {};
      for (const d of p.devices) devices[d] = { state: 'waiting', done: 0, total: 0, command: '', error: '' };
      return { ...run, scope: p.scope, devices, order: [...p.devices] };
    }
    case 'device': {
      const was = run.devices[p.device] ?? { state: 'waiting', done: 0, total: 0, command: '', error: '' };
      const state = (p.state || was.state) as DeviceState;
      const next = { state, done: p.total ? p.done : was.done, total: p.total || was.total, command: p.command || was.command, error: p.error || was.error };
      return { ...run, devices: { ...run.devices, [p.device]: next }, order: run.order.includes(p.device) ? run.order : [...run.order, p.device] };
    }
    case 'finished':
      return { ...run, finished: true, summary: p.summary };
    case 'failed':
      return { ...run, finished: true, error: p.error };
    default:
      return run;
  }
}

/** How far the run has got, 0 to 1: finished devices count whole,
 *  running ones by their commands. */
export function fraction(run: LiveRun): number {
  const all = run.order.map((d) => run.devices[d]).filter((d): d is NonNullable<typeof d> => !!d);
  if (all.length === 0) return run.finished ? 1 : 0;
  const parts = all.map((d) => (d.state === 'done' || d.state === 'unreachable' || d.state === 'stopped' ? 1 : d.total ? d.done / d.total : 0));
  return parts.reduce((a, b) => a + b, 0) / all.length;
}

/** Where a plan step stands, from its proposal in the change record. */
export type StepState = 'manual' | 'ready' | 'pending' | 'running' | 'done' | 'denied' | 'failed';

export function stepState(step: DiagnosePlanStep, changes: Record<string, Change>): StepState {
  if (step.status === 'manual') return 'manual';
  const c = step.proposalId ? changes[step.proposalId] : undefined;
  if (!c) return step.status === 'proposed' ? 'pending' : 'ready';
  switch (c.status) {
    case 'pending':
      return 'pending';
    case 'running':
      return 'running';
    case 'done':
      return 'done';
    case 'failed':
      return 'failed';
    default:
      return 'denied';
  }
}

/** The proposals of a report still waiting, oldest first: what "Review
 *  fix plan" opens. */
export function waitingProposals(report: DiagnoseReport, changes: Record<string, Change>): string[] {
  return report.plan
    .map((s) => (s.proposalId ? changes[s.proposalId] : undefined))
    .filter((c): c is Change => !!c && c.status === 'pending')
    .sort((a, b) => a.createdMs - b.createdMs)
    .map((c) => c.id);
}

/** Whether a step can be proposed again from the report. */
export function canProposeAgain(state: StepState): boolean {
  return state === 'denied' || state === 'failed';
}

export const SEVERITY_ORDER: DiagnoseSeverity[] = ['critical', 'warning', 'info'];

/** Findings grouped by severity, worst first, each group in device order. */
export function bySeverity(report: DiagnoseReport): { severity: DiagnoseSeverity; findings: DiagnoseReport['findings'] }[] {
  return SEVERITY_ORDER.map((severity) => ({ severity, findings: report.findings.filter((f) => f.severity === severity).sort((a, b) => a.device.localeCompare(b.device)) })).filter((g) => g.findings.length > 0);
}

/** The arguments of each Diagnose button. */
export const diagnoseArgs = {
  project: (): DiagnoseArgs => ({ scope: 'project' }),
  site: (site: string): DiagnoseArgs => ({ scope: 'site', site }),
  devices: (targets: string[]): DiagnoseArgs => ({ scope: 'devices', targets }),
  /** A link: both of its ends. */
  link: (a: string, b: string): DiagnoseArgs => ({ scope: 'devices', targets: a === b ? [a] : [a, b] }),
  path: (src: string, dst: string, port?: number | null): DiagnoseArgs => ({ scope: 'path', src, dst, port: port ?? null }),
};

/** What a report is about, in a few words. */
export function scopeWords(report: DiagnoseReport): string {
  const s = report.scope;
  switch (s.kind) {
    case 'project':
      return 'the whole project';
    case 'site':
      return `site ${s.site ?? ''}`;
    case 'devices':
      return (s.devices ?? []).join(', ');
    case 'path':
      return `${s.src ?? ''} → ${s.dst ?? ''}`;
  }
}

/** The question the chat is asked about a report. */
export function explainPrompt(report: DiagnoseReport): string {
  const top = report.causes[0];
  return `Explain diagnosis ${report.id} of ${scopeWords(report)}${top ? `: is "${top.title}" the cause, and what would confirm it?` : ' and what, if anything, needs attention.'}`;
}
