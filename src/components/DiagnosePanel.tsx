import { useCallback, useEffect, useMemo, useState } from 'react';

import { t } from '../i18n';
import { applyProgress, bySeverity, canProposeAgain, explainPrompt, fraction, startRun, stepState, waitingProposals, type LiveRun } from '../lib/diagnose';
import { saveExport, slug } from '../lib/exports';
import { ipc, isDesktop, type DiagnoseArgs, type DiagnoseReport, type DiagnoseScopeKind, type DiagnoseSummary } from '../lib/ipc';
import { activePage } from '../lib/pages';
import { useStore } from '../state/store';
import { IconButton } from './chromeIcons';

/**
 * Diagnose network: choose what to diagnose, watch the read-only sweep
 * device by device (and stop it), then read the report — the likely root
 * causes with their evidence, the findings, what passed and what could not
 * be checked — and review its fix plan, whose steps wait in the approval
 * dialog as proposals. Rust runs the sweep and proposes; this panel never
 * changes anything itself.
 */
export function DiagnosePanel() {
  const open = useStore((s) => s.diagnoseOpen);
  const request = useStore((s) => s.diagnoseRequest);
  const setOpen = useStore((s) => s.setDiagnoseOpen);
  const changes = useStore((s) => s.aiChanges);
  const openApproval = useStore((s) => s.openApproval);
  const projectId = useStore((s) => s.meta?.id ?? null);
  const projectName = useStore((s) => s.meta?.name ?? 'project');
  const doc = useStore((s) => s.doc);
  const [args, setArgs] = useState<DiagnoseArgs>({ scope: 'project', depth: 'quick' });
  const [run, setRun] = useState<LiveRun | null>(null);
  const [report, setReport] = useState<DiagnoseReport | null>(null);
  const [history, setHistory] = useState<DiagnoseSummary[]>([]);
  const [problem, setProblem] = useState<string | null>(null);

  const devices = useMemo(() => {
    const out: { label: string; site: string; selected: boolean }[] = [];
    for (const page of doc.pages) {
      for (const n of page.nodes) {
        if (n.type !== 'device') continue;
        const d = n.data as { label?: string; site?: string };
        out.push({ label: String(d.label ?? ''), site: String(d.site ?? ''), selected: page.id === activePage(doc).id && !!n.selected });
      }
    }
    return out;
  }, [doc]);
  const sites = useMemo(() => [...new Set(devices.map((d) => d.site).filter(Boolean))].sort(), [devices]);
  const selected = devices.filter((d) => d.selected).map((d) => d.label);

  const loadHistory = useCallback(() => {
    void ipc.diagnoseList(30).then(setHistory).catch(() => setHistory([]));
  }, []);

  const showReport = useCallback(async (id: string) => {
    try {
      setReport(await ipc.diagnoseGet(id));
      setProblem(null);
    } catch (e) {
      setProblem(String(e));
    }
  }, []);

  const start = useCallback(async (a: DiagnoseArgs) => {
    setProblem(null);
    setReport(null);
    try {
      const id = await ipc.diagnoseStart(a);
      setRun(startRun(id));
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    if (open) loadHistory();
  }, [open, projectId, loadHistory]);

  // A button elsewhere (a device, a link, two devices) asks for a run.
  useEffect(() => {
    if (!request) return;
    setArgs({ depth: 'quick', ...request.args });
    if (request.start) void start({ depth: 'quick', ...request.args });
    else {
      setRun(null);
      setReport(null);
    }
  }, [request, start]);

  useEffect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    void ipc
      .onDiagnose((p) => {
        if (p.projectId && p.projectId !== useStore.getState().meta?.id) return;
        setRun((r) => {
          if (!r) return r;
          const next = applyProgress(r, p);
          if (p.runId === r.runId && p.kind === 'finished') {
            void showReport(p.runId);
            loadHistory();
          }
          return next;
        });
        if (p.kind === 'plan') {
          setReport((rep) => {
            if (rep && rep.id === p.runId) void showReport(p.runId);
            return rep;
          });
        }
      })
      .then((s) => {
        if (gone) s();
        else stop = s;
      });
    return () => {
      gone = true;
      stop?.();
    };
  }, [showReport, loadHistory]);

  if (!open || !projectId) return null;
  const running = run && !run.finished;
  const kinds: DiagnoseScopeKind[] = ['project', 'site', 'devices', 'path'];
  const canRun =
    isDesktop &&
    !running &&
    (args.scope === 'project' || (args.scope === 'site' && !!args.site) || (args.scope === 'devices' && (args.targets?.length ?? selected.length) > 0) || (args.scope === 'path' && !!args.src?.trim() && !!args.dst?.trim()));
  const runArgs = (): DiagnoseArgs => (args.scope === 'devices' && !args.targets?.length ? { ...args, targets: selected } : args);

  const exportMd = async (r: DiagnoseReport) => {
    try {
      await saveExport(`${slug(projectName)}-diagnosis-${r.id}.md`, await ipc.diagnoseExportMd(r.id), 'text/markdown');
    } catch (e) {
      setProblem(String(e));
    }
  };
  const exportPdf = async (r: DiagnoseReport) => {
    try {
      await saveExport(`${slug(projectName)}-diagnosis-${r.id}.pdf`, await ipc.diagnoseExportPdf(r.id), 'application/pdf');
    } catch (e) {
      setProblem(String(e));
    }
  };

  return (
    <section className="cv-diagnose" data-region="diagnose" aria-label={t('diagnose.title')}>
      <header className="cv-diagnose-head">
        <strong>{t('diagnose.title')}</strong>
        <span className="cv-diagnose-tools">
          {(report || run) && !running && (
            <button type="button" className="cv-btn cv-btn-small" data-action="diagnose-new" onClick={() => { setRun(null); setReport(null); }}>
              {t('diagnose.new')}
            </button>
          )}
          <IconButton icon="x" label={t('diagnose.close')} onClick={() => setOpen(false)} region="diagnose-close" />
        </span>
      </header>
      <div className="cv-diagnose-body">
        {problem && <p className="cv-help cv-update-problem" role="alert" data-hint="diagnose-problem">{problem}</p>}

        {!run && !report && (
          <form className="cv-diagnose-form" onSubmit={(e) => { e.preventDefault(); if (canRun) void start(runArgs()); }}>
            <fieldset>
              <legend>{t('diagnose.scope')}</legend>
              {kinds.map((k) => (
                <label key={k} className="cv-check">
                  <input type="radio" name="diagnose-scope" value={k} data-field={`diagnose-scope-${k}`} checked={args.scope === k} onChange={() => setArgs({ ...args, scope: k, targets: k === 'devices' ? selected : null })} />
                  {t(`diagnose.scope.${k}`)}
                </label>
              ))}
            </fieldset>
            {args.scope === 'site' && (
              <label className="cv-field">
                {t('diagnose.site')}
                <select className="cv-input" data-field="diagnose-site" value={args.site ?? ''} onChange={(e) => setArgs({ ...args, site: e.target.value })}>
                  <option value="" />
                  {sites.map((s) => <option key={s} value={s}>{s}</option>)}
                </select>
              </label>
            )}
            {args.scope === 'devices' && (
              <p className="cv-help" data-hint="diagnose-targets">
                {(args.targets?.length ? args.targets : selected).join(', ') || t('diagnose.noSelection')}
              </p>
            )}
            {args.scope === 'path' && (
              <>
                <label className="cv-field">{t('diagnose.src')}<input className="cv-input" data-field="diagnose-src" list="cv-diagnose-devices" value={args.src ?? ''} onChange={(e) => setArgs({ ...args, src: e.target.value })} /></label>
                <label className="cv-field">{t('diagnose.dst')}<input className="cv-input" data-field="diagnose-dst" list="cv-diagnose-devices" value={args.dst ?? ''} onChange={(e) => setArgs({ ...args, dst: e.target.value })} /></label>
                <label className="cv-field">{t('diagnose.port')}<input className="cv-input" data-field="diagnose-port" inputMode="numeric" value={args.port ?? ''} onChange={(e) => { const n = Number.parseInt(e.target.value, 10); setArgs({ ...args, port: Number.isFinite(n) && n > 0 && n < 65536 ? n : null }); }} /></label>
                <datalist id="cv-diagnose-devices">{devices.map((d) => <option key={d.label} value={d.label} />)}</datalist>
              </>
            )}
            <label className="cv-field">
              {t('diagnose.depth')}
              <select className="cv-input" data-field="diagnose-depth" value={args.depth ?? 'quick'} onChange={(e) => setArgs({ ...args, depth: e.target.value as 'quick' | 'full' })}>
                <option value="quick">{t('diagnose.depth.quick')}</option>
                <option value="full">{t('diagnose.depth.full')}</option>
              </select>
            </label>
            <label className="cv-field">
              {t('diagnose.symptom')}
              <input className="cv-input" data-field="diagnose-symptom" value={args.symptom ?? ''} title={t('diagnose.symptomHint')} onChange={(e) => setArgs({ ...args, symptom: e.target.value })} />
            </label>
            <p className="cv-help">{t('diagnose.readOnly')}</p>
            <button type="submit" className="cv-btn cv-btn-primary" data-action="diagnose-run" disabled={!canRun}>{t('diagnose.run')}</button>
            <h4>{t('diagnose.history')}</h4>
            {history.length === 0 && <p className="cv-help">{t('diagnose.noHistory')}</p>}
            <ul className="cv-diagnose-history" data-region="diagnose-history">
              {history.map((h) => (
                <li key={h.id}>
                  <button type="button" className="cv-link" data-action="diagnose-open-report" onClick={() => void showReport(h.id)}>
                    {new Date(h.startedMs).toLocaleString()} · {h.scope} · {h.critical} critical, {h.warning} warning{h.topCause ? ` · ${h.topCause}` : ''}
                  </button>
                </li>
              ))}
            </ul>
          </form>
        )}

        {run && !report && (
          <div className="cv-diagnose-progress" data-region="diagnose-progress">
            <p>{t('diagnose.running', { scope: run.scope || '…' })}</p>
            <progress max={1} value={fraction(run)} aria-label={t('diagnose.running', { scope: run.scope })} />
            <ul>
              {run.order.map((d) => {
                const s = run.devices[d];
                return (
                  <li key={d} data-device={d} data-state={s?.state} className={`is-${s?.state ?? 'waiting'}`}>
                    <strong>{d}</strong> {t(`diagnose.state.${s?.state ?? 'waiting'}`)}
                    {s?.state === 'running' && s.total > 0 && <> · {s.done}/{s.total} · <code>{s.command}</code></>}
                    {s?.error && <span className="cv-help"> — {s.error}</span>}
                  </li>
                );
              })}
            </ul>
            {run.error && <p className="cv-help cv-update-problem" role="alert">{t('diagnose.failed', { error: run.error })}</p>}
            {running && (
              <button type="button" className="cv-btn cv-btn-stop" data-action="diagnose-cancel" onClick={() => void ipc.diagnoseCancel(run.runId)}>
                {t('diagnose.cancel')}
              </button>
            )}
          </div>
        )}

        {report && <ReportView report={report} changes={changes} onReview={(id) => openApproval(id)} onExportMd={() => void exportMd(report)} onExportPdf={() => void exportPdf(report)} onProposeAgain={(seq) => void ipc.diagnosePropose(report.id, seq).catch((e) => setProblem(String(e)))} />}
      </div>
    </section>
  );
}

function ReportView({ report, changes, onReview, onExportMd, onExportPdf, onProposeAgain }: {
  report: DiagnoseReport;
  changes: ReturnType<typeof useStore.getState>['aiChanges'];
  onReview: (id: string | null) => void;
  onExportMd: () => void;
  onExportPdf: () => void;
  onProposeAgain: (seq: number) => void;
}) {
  const waiting = waitingProposals(report, changes);
  const titleOf = (id: string) => report.findings.find((f) => f.id === id);
  return (
    <div className="cv-diagnose-report" data-region="diagnose-report" data-run={report.id}>
      <div className="cv-diagnose-actions">
        <button type="button" className="cv-btn cv-btn-small" data-action="diagnose-export-md" onClick={onExportMd}>{t('diagnose.exportMd')}</button>
        <button type="button" className="cv-btn cv-btn-small" data-action="diagnose-export-pdf" onClick={onExportPdf}>{t('diagnose.exportPdf')}</button>
        <button type="button" className="cv-btn cv-btn-small" data-action="diagnose-explain" onClick={() => useStore.getState().askAi(null, explainPrompt(report))}>{t('diagnose.explain')}</button>
      </div>
      <h4>{t('diagnose.summary')}</h4>
      <ul className="cv-diagnose-summary" data-region="diagnose-summary">{report.summary.map((l, i) => <li key={i}>{l}</li>)}</ul>
      {report.cancelled && <p className="cv-help">{t('diagnose.stoppedNote')}</p>}

      <h4>{t('diagnose.causes')}</h4>
      {report.causes.length === 0 && <p className="cv-help">{t('diagnose.noCauses')}</p>}
      <ol className="cv-diagnose-causes">
        {report.causes.map((c) => (
          <li key={c.id} data-cause={c.id.split('|')[0]} className={`is-${c.severity}`}>
            <strong>{c.title}</strong> <span className="cv-help">{t('diagnose.confidence', { pct: Math.round(c.confidence * 100) })}</span>
            <p>{c.why}</p>
            {c.supporting.length > 0 && (
              <details><summary>{t('diagnose.supporting')}</summary><pre className="cv-mono">{c.supporting.join('\n')}</pre></details>
            )}
            {c.rulingOut.length > 0 && (
              <details><summary>{t('diagnose.rulingOut')}</summary><ul>{c.rulingOut.map((r, i) => <li key={i}>{r}</li>)}</ul></details>
            )}
          </li>
        ))}
      </ol>

      <h4>{t('diagnose.plan')}</h4>
      {report.plan.length === 0 && <p className="cv-help">{t('diagnose.noPlan')}</p>}
      {report.plan.length > 0 && (
        <>
          <button type="button" className="cv-btn cv-btn-primary" data-action="diagnose-review" disabled={waiting.length === 0} onClick={() => onReview(waiting[0] ?? null)}>
            {t('diagnose.review')}{waiting.length > 0 ? ` (${waiting.length})` : ''}
          </button>
          <p className="cv-help">{t('diagnose.reviewHint')}</p>
        </>
      )}
      <ol className="cv-diagnose-plan" data-region="diagnose-plan">
        {report.plan.map((s) => {
          const state = stepState(s, changes);
          return (
            <li key={s.seq} data-step={s.seq} data-state={state}>
              <strong>{s.title}</strong> <span className="cv-help">· {s.device} · {t(`diagnose.step.${state}`)}</span>
              {s.commands.length > 0 && <pre className="cv-mono">{s.commands.join('\n')}</pre>}
              {s.risk && <p><em>{t('diagnose.risk')}:</em> {s.risk}</p>}
              <p><em>{t('diagnose.impact')}:</em> {s.impact}</p>
              {s.verify.length > 0 && <p><em>{t('diagnose.verify')}:</em> <code>{s.verify.join(', ')}</code></p>}
              <p className="cv-help"><em>{t('diagnose.rollback')}:</em> {s.rollback}</p>
              {s.note && <p className="cv-help">{s.note}</p>}
              {state === 'pending' && s.proposalId && (
                <button type="button" className="cv-btn cv-btn-small" data-action="diagnose-open-step" onClick={() => onReview(s.proposalId)}>{t('aiChat.openApproval')}</button>
              )}
              {canProposeAgain(state) && (
                <button type="button" className="cv-btn cv-btn-small" data-action="diagnose-propose-again" onClick={() => onProposeAgain(s.seq)}>{t('diagnose.proposeAgain')}</button>
              )}
            </li>
          );
        })}
      </ol>

      {report.path && (
        <>
          <h4>{t('diagnose.pathTitle', { src: report.path.src, dst: report.path.dst })}</h4>
          <ol className="cv-diagnose-path" data-region="diagnose-path">
            {report.path.hops.map((h, i) => (
              <li key={i} className={report.path?.firstBreak === i ? 'is-break' : ''}>
                <strong>{h.device}</strong> <span className="cv-help">({h.inPort || '-'} → {h.outPort || '-'}){report.path?.firstBreak === i ? ` — ${t('diagnose.breaksHere')}` : ''}</span>
                <ul>{h.checks.map((c, j) => <li key={j} className={c.ok ? 'is-ok' : 'is-bad'}>{c.what}: {c.detail}</li>)}</ul>
              </li>
            ))}
          </ol>
          {report.path.note && <p className="cv-help">{report.path.note}</p>}
        </>
      )}

      <h4>{t('diagnose.findings')}</h4>
      {bySeverity(report).map((g) => (
        <details key={g.severity} open={g.severity === 'critical'} data-severity={g.severity}>
          <summary>{g.severity} ({g.findings.length})</summary>
          <ul className="cv-diagnose-findings">
            {g.findings.map((f) => (
              <li key={f.id} data-check={f.check}>
                <strong>{f.device}</strong> — {f.says}
                {f.evidence.length > 0 && <pre className="cv-mono">{f.evidence.join('\n')}</pre>}
              </li>
            ))}
          </ul>
        </details>
      ))}

      <details>
        <summary>{t('diagnose.passed')} ({report.passed.length})</summary>
        <ul>{report.passed.map((p) => <li key={p.check}>{p.title} — {p.says} <span className="cv-help">({p.devices.join(', ')})</span></li>)}</ul>
      </details>
      <details open={report.notChecked.some((n) => n.check === '*')}>
        <summary>{t('diagnose.notChecked')} ({report.notChecked.length})</summary>
        <ul data-region="diagnose-not-checked">{report.notChecked.map((n, i) => <li key={i}><strong>{n.device}</strong> {n.check === '*' ? '' : `${n.check}: `}{n.why}</li>)}</ul>
      </details>
      {report.diff && (
        <>
          <h4>{t('diagnose.diff')}</h4>
          <p>{t('diagnose.diffLine', { new: report.diff.new.length, resolved: report.diff.resolved.length, unchanged: report.diff.unchanged })}</p>
          <ul>
            {report.diff.new.map((id) => <li key={id}>+ {titleOf(id)?.device} — {titleOf(id)?.says}</li>)}
            {report.diff.resolved.map((x, i) => <li key={i}>− {x}</li>)}
          </ul>
        </>
      )}
      <p className="cv-help">{t('diagnose.playbooks', { list: report.playbooks.join(', ') })}</p>
    </div>
  );
}
