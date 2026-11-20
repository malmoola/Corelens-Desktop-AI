import { useEffect, useRef, useState } from 'react';

import { t } from '../i18n';
import { CONSOLE_SPEEDS, FRAMINGS, describeLine, lineFrom, pickPort, type Picked } from '../lib/consoleLine';
import { ipc, type ConsoleLine, type ConsolePort } from '../lib/ipc';
import { useStore } from '../state/store';
import { openConsole } from './sshActions';

/** How often the port list is looked at again while the form is open, so a
 *  cable plugged in now is noticed without anyone pressing anything. */
const LOOK_AGAIN_MS = 1500;

/**
 * Open a console: the port, found by itself, and the line.
 *
 * The ports are listed with console cables first and the best one already
 * chosen. While the form is open the list is read again every second and
 * a half; a port that appears is the cable just plugged in, and it is chosen
 * and said so. A choice made by hand is never taken back by that.
 */
export function ConsoleForm() {
  const ask = useStore((s) => s.consoleAsk);
  const terminal = useStore((s) => s.settings.terminal);
  const setTerminal = useStore((s) => s.setTerminalSettings);
  const [ports, setPorts] = useState<ConsolePort[] | null>(null);
  const [chosen, setChosen] = useState<string>('');
  const [picked, setPicked] = useState<Picked | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  const byHand = useRef(false);
  const handChoice = useRef('');
  const last = useRef<ConsolePort[] | undefined>(undefined);

  useEffect(() => {
    if (!ask) return;
    let stopped = false;
    byHand.current = false;
    last.current = undefined;
    const look = async () => {
      let now: ConsolePort[];
      try {
        now = await ipc.consolePorts();
      } catch (e: unknown) {
        if (!stopped) setProblem(e instanceof Error ? e.message : String(e));
        return;
      }
      if (stopped) return;
      const pick = pickPort(now, last.current);
      last.current = now;
      setPorts(now);
      // A port chosen by hand that has gone is no longer a choice.
      if (byHand.current && !now.some((p) => p.name === handChoice.current)) byHand.current = false;
      // A port that has just appeared beats a choice made by hand: plugging
      // the cable in is the clearer of the two. Nothing else does.
      if (pick.why === 'plugged' || !byHand.current) {
        byHand.current = false;
        setChosen(pick.name ?? '');
        setPicked(pick);
      }
    };
    void look();
    const every = window.setInterval(() => void look(), LOOK_AGAIN_MS);
    return () => {
      stopped = true;
      window.clearInterval(every);
    };
  }, [ask]);

  if (!ask) return null;

  const line: ConsoleLine = lineFrom(terminal.consoleSpeed, terminal.consoleFraming, terminal.consoleFlow);
  const chosenPort = ports?.find((p) => p.name === chosen);
  const close = () => {
    useStore.getState().askConsole(null);
    setProblem(null);
    setPorts(null);
    setChosen('');
    setPicked(null);
  };
  const go = async () => {
    if (!chosen) return;
    setOpening(true);
    setProblem(null);
    const why = await openConsole(chosen, line, ask);
    setOpening(false);
    if (why) setProblem(why);
    else close();
  };

  const said = (() => {
    if (ports === null) return t('console.looking');
    if (!picked) return null;
    const p = picked.name ? ports.find((x) => x.name === picked.name) : undefined;
    switch (picked.why) {
      case 'plugged': return t('console.foundPlugged', { port: picked.name ?? '', what: p?.description ?? '' });
      case 'console': return t('console.foundCable', { port: picked.name ?? '', what: p?.description ?? '' });
      case 'usb': return t('console.foundUsb', { port: picked.name ?? '', what: p?.description ?? '' });
      case 'only': return t('console.foundOnly', { port: picked.name ?? '' });
      case 'none': return t('console.noPorts');
      case 'unsure': return t('console.unsure');
    }
  })();

  return (
    <div className="cv-ssh-send cv-console-form" data-region="console-open" role="group" aria-label={t('console.title')}>
      <strong>{ask.label ? t('console.titleFor', { name: ask.label }) : t('console.title')}</strong>
      {said && <p className="cv-help" data-region="console-found">{said}</p>}
      <div className="cv-console-fields">
        <label className="cv-field">
          <span>{t('console.port')}</span>
          <select className="cv-input" value={chosen} data-field="console-port"
            onChange={(e) => { byHand.current = true; handChoice.current = e.target.value; setChosen(e.target.value); setPicked(null); }}>
            {!chosen && <option value="">{t('console.choosePort')}</option>}
            {(ports ?? []).map((p) => (
              <option key={p.name} value={p.name}>
                {p.console ? t('console.portCable', { port: p.name, what: p.description }) : t('console.portOther', { port: p.name, what: p.description })}
              </option>
            ))}
          </select>
        </label>
        <label className="cv-field cv-field-narrow">
          <span>{t('console.speed')}</span>
          <select className="cv-input" value={line.speed} data-field="console-speed"
            onChange={(e) => setTerminal({ consoleSpeed: Number(e.target.value) })}>
            {CONSOLE_SPEEDS.map((s) => <option key={s} value={s}>{s}</option>)}
          </select>
        </label>
        <label className="cv-field cv-field-narrow">
          <span>{t('console.framing')}</span>
          <select className="cv-input" value={terminal.consoleFraming} data-field="console-framing"
            onChange={(e) => setTerminal({ consoleFraming: e.target.value })}>
            {FRAMINGS.map((f) => <option key={f} value={f}>{f}</option>)}
          </select>
        </label>
        <label className="cv-field cv-field-narrow">
          <span>{t('console.flow')}</span>
          <select className="cv-input" value={line.flow} data-field="console-flow"
            onChange={(e) => setTerminal({ consoleFlow: e.target.value as ConsoleLine['flow'] })}>
            <option value="none">{t('console.flowNone')}</option>
            <option value="hardware">{t('console.flowHardware')}</option>
            <option value="software">{t('console.flowSoftware')}</option>
          </select>
        </label>
      </div>
      {chosenPort?.kind === 'bluetooth' && <p className="cv-problem">{t('console.bluetooth')}</p>}
      {problem && <p className="cv-problem">{problem}</p>}
      <div className="cv-keep-cred-held">
        <button type="button" className="cv-btn cv-btn-small cv-btn-start" disabled={!chosen || opening}
          onClick={() => void go()}>
          {t('console.open', { port: chosen || '…', line: describeLine(line) })}
        </button>
        <button type="button" className="cv-btn cv-btn-small" onClick={close}>{t('ssh.sendCancel')}</button>
      </div>
    </div>
  );
}
