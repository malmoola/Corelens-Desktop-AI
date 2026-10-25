import { useEffect, useMemo, useState } from 'react';

import { t } from '../i18n';
import { approvable, canRollBack, decision, missing, needsTypedName, nextPending, secondsLeft, tone } from '../lib/changes';
import { ipc, type Change, type ChangeDiffLine, type ChangeTarget } from '../lib/ipc';
import { useStore } from '../state/store';
import { ChromeIcon } from './chromeIcons';

/**
 * Where a proposed configuration change is decided — the only place.
 *
 * A proposal from an MCP client or the chat arrives on `corelens://change`
 * and opens this dialog: per device the exact commands, the predicted
 * diff against its running configuration, the risk, and the rollback the
 * platform offers. A device whose lines can cut it off needs its name
 * typed. Approve per device or all at once with the whole list in view;
 * Deny, or let the clock run out, and nothing is sent. Once decided, the
 * dialog follows the change through its push, verification and rollback.
 */
export function ChangeApproval() {
  const changes = useStore((s) => s.aiChanges);
  const apply = useStore((s) => s.applyAiChange);
  const asked = useStore((s) => s.approvalOpen);
  const openApproval = useStore((s) => s.openApproval);
  const [shown, setShown] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    // Removed even when it arrives after cleanup (React mounts twice in
    // development), so no change is reported twice.
    let stop: (() => void) | undefined;
    let gone = false;
    void ipc.changeList(50).then((all) => all.forEach(apply)).catch(() => undefined);
    void ipc.onChange((c) => apply(c)).then((s) => {
      if (gone) s();
      else stop = s;
    });
    return () => {
      gone = true;
      stop?.();
    };
  }, [apply]);

  // A new proposal takes the dialog unless one is already being looked at.
  const pending = nextPending(changes);
  useEffect(() => {
    if (asked) setShown(asked);
    else if (!shown && pending) setShown(pending.id);
  }, [asked, pending, shown]);

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  const change = shown ? changes[shown] : undefined;
  if (!change) return null;
  const close = () => {
    // Reviewing a fix plan goes on to its next step still waiting.
    const next = change.source === 'diagnose' && change.status !== 'pending' ? nextPending(changes, true) : null;
    setShown(next?.id ?? null);
    openApproval(next?.id ?? null);
  };
  return <Dialog key={change.id} change={change} now={now} onClose={close} />;
}

function Dialog({ change, now, onClose }: { change: Change; now: number; onClose: () => void }) {
  const [ticked, setTicked] = useState<Set<number>>(() => new Set(change.targets.filter((t) => !needsTypedName(t)).map((t) => t.seq)));
  const [typed, setTyped] = useState<Record<number, string>>({});
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = change.status === 'pending';
  const ok = useMemo(() => approvable(change, ticked, typed), [change, ticked, typed]);
  const stillToType = missing(change, ticked, typed);
  const allTicked = new Set(change.targets.map((t) => t.seq));
  const allOk = approvable(change, allTicked, typed);

  const decide = async (approve: number[]) => {
    setBusy(true);
    setProblem(null);
    try {
      const d = decision(approve, typed);
      await ipc.changeDecide(change.id, d.approve, d.typed, approve.length === 0 ? t('changes.deniedNote') : null);
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const rollBack = async (seq: number) => {
    setBusy(true);
    setProblem(null);
    try {
      useStore.getState().applyAiChange(await ipc.changeRollback(change.id, seq));
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const toggle = (seq: number, on: boolean) => {
    const next = new Set(ticked);
    if (on) next.add(seq);
    else next.delete(seq);
    setTicked(next);
  };

  return (
    <div className="cv-modal-backdrop" role="presentation">
      <div className="cv-modal cv-change-dialog" role="dialog" aria-modal="true" aria-label={t('changes.title')} data-region="change-approval" data-change={change.id} data-status={change.status}>
        <div className="cv-change-head">
          <h2>{change.kind === 'save' ? t('changes.titleSave') : t('changes.title')}</h2>
          {pending && (
            <span className="cv-change-clock" data-hint="change-deadline">
              {t('changes.deadline', { seconds: secondsLeft(change, now) })}
            </span>
          )}
        </div>
        <p className="cv-change-reason" data-field="change-reason">
          <strong>{t('changes.reason')}</strong> {change.reason}
        </p>
        <p className="cv-help" data-hint="change-from">{t('changes.from', { proposer: change.proposer, id: change.id })}</p>
        {!pending && <p className={`cv-change-status is-${change.status}`} data-hint="change-status">{t(`changes.status.${change.status}`)}{change.note ? ` — ${change.note}` : ''}</p>}
        <p className="cv-help cv-change-sim">{t('changes.simulatorOnly')}</p>
        <div className="cv-change-targets">
          {change.targets.map((target) => (
            <TargetCard
              key={target.seq}
              target={target}
              pending={pending}
              ticked={ticked.has(target.seq)}
              typed={typed[target.seq] ?? ''}
              busy={busy}
              onTick={(on) => toggle(target.seq, on)}
              onType={(v) => setTyped({ ...typed, [target.seq]: v })}
              onRollBack={() => void rollBack(target.seq)}
            />
          ))}
        </div>
        {problem && <p className="cv-help cv-update-problem" role="alert">{problem}</p>}
        {pending && stillToType.length > 0 && <p className="cv-help cv-change-type-hint">{t('changes.typeToApprove', { devices: stillToType.join(', ') })}</p>}
        <div className="cv-modal-actions">
          {pending ? (
            <>
              <button type="button" className="cv-btn is-danger" data-action="change-deny" disabled={busy} onClick={() => void decide([])}>
                {t('changes.deny')}
              </button>
              {change.targets.length > 1 && (
                <button type="button" className="cv-btn" data-action="change-approve-all" disabled={busy || allOk.length !== change.targets.length} onClick={() => void decide(allOk)}>
                  {t('changes.approveAll', { count: change.targets.length })}
                </button>
              )}
              <button type="button" className="cv-btn cv-btn-primary" data-action="change-approve" disabled={busy || ok.length === 0} onClick={() => void decide(ok)}>
                {change.targets.length > 1 ? t('changes.approveSelected', { count: ok.length }) : t('changes.approve')}
              </button>
            </>
          ) : (
            <button type="button" className="cv-btn" data-action="change-close" onClick={onClose}>
              {t('changes.close')}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function Diff({ lines }: { lines: ChangeDiffLine[] }) {
  if (lines.length === 0) return <p className="cv-help">{t('changes.noDiff')}</p>;
  return (
    <pre className="cv-mono cv-change-diff" data-field="change-diff">
      {lines.map((d, i) => (
        <span key={i} className={d.mark === '+' ? 'is-add' : d.mark === '-' ? 'is-del' : ''}>
          {d.mark}
          {d.text}
          {'\n'}
        </span>
      ))}
    </pre>
  );
}

function TargetCard({
  target,
  pending,
  ticked,
  typed,
  busy,
  onTick,
  onType,
  onRollBack,
}: {
  target: ChangeTarget;
  pending: boolean;
  ticked: boolean;
  typed: string;
  busy: boolean;
  onTick: (on: boolean) => void;
  onType: (v: string) => void;
  onRollBack: () => void;
}) {
  const hard = needsTypedName(target);
  return (
    <section className={`cv-change-target is-${target.assessment.class}`} data-device={target.device} data-target-status={target.status}>
      <div className="cv-change-target-head">
        {pending ? (
          <label className="cv-check">
            <input type="checkbox" data-field="change-tick" checked={ticked} onChange={(e) => onTick(e.target.checked)} />
            <strong>{target.device}</strong>
          </label>
        ) : (
          <strong>{target.device}</strong>
        )}
        <span className="cv-help">
          {target.address} · {target.platformName}
        </span>
        <span className={`cv-change-risk is-${target.assessment.class}`} data-hint="change-risk">
          {t(`changes.risk.${target.assessment.class}`)}
        </span>
        {!pending && <span className={`cv-change-tstatus is-${tone(target.status)}`}>{t(`changes.target.${target.status}`)}</span>}
      </div>
      <p className="cv-help">{target.assessment.summary}</p>
      {hard && (
        <div className="cv-change-hard" role="note" data-region="change-hard-block">
          <p>
            <ChromeIcon name="alert" /> {t('changes.hardTitle')}
          </p>
          <ul>
            {target.assessment.hardBlocks.map((h, i) => (
              <li key={i}>
                <code>{h.command}</code> — {h.risk}
              </li>
            ))}
          </ul>
          {pending && (
            <label className="cv-field">
              <span>{t('changes.typeName', { device: target.device })}</span>
              <input className="cv-input" data-field="change-typed" value={typed} autoComplete="off" spellCheck={false} onChange={(e) => onType(e.target.value)} placeholder={target.device} />
            </label>
          )}
        </div>
      )}
      <h3>{t('changes.commands')}</h3>
      <pre className="cv-mono cv-change-commands" data-field="change-commands">{target.commands.join('\n')}</pre>
      <h3>{target.actual.length ? t('changes.actualDiff') : target.predictedIsModel ? t('changes.predictedDiff') : t('changes.asTyped')}</h3>
      <Diff lines={target.actual.length ? target.actual : target.predicted} />
      {target.verifyCommands.length > 0 && (
        <>
          <h3>{t('changes.verifyWith')}</h3>
          <pre className="cv-mono cv-change-commands">{target.verifyCommands.join('\n')}</pre>
        </>
      )}
      <p className="cv-help">
        <strong>{t('changes.rollback')}</strong> {target.rollbackPlan}
      </p>
      {target.message && <p className={`cv-change-message is-${tone(target.status)}`} data-hint="change-message">{target.message}</p>}
      {target.verification.length > 0 && (
        <ul className="cv-change-checks">
          {target.verification.map((v, i) => (
            <li key={i} className={v.ok ? 'is-ok' : 'is-bad'}>
              <details>
                <summary>
                  <ChromeIcon name={v.ok ? 'check' : 'x'} /> <code>{v.command}</code>
                </summary>
                <pre className="cv-mono cv-ai-result">{v.output}</pre>
              </details>
            </li>
          ))}
        </ul>
      )}
      {canRollBack(target.status) && (
        <button type="button" className="cv-btn is-danger" data-action="change-rollback" disabled={busy} onClick={onRollBack}>
          {t('changes.rollBack')}
        </button>
      )}
    </section>
  );
}
