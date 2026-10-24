import { useCallback, useEffect, useState } from 'react';

import { t } from '../i18n';
import { ipc, isDesktop, type McpActivity, type McpStatus } from '../lib/ipc';
import { saveExport, slug } from '../lib/exports';
import { useStore } from '../state/store';

/**
 * Settings → AI: whether Claude (Claude Code or Claude Desktop, through
 * MCP) may use Corelens, how to connect it, and what it has been given.
 *
 * Off until switched on. While off nothing listens; while on, a client
 * started as `corelens-desktop-ai --mcp` can read the open project and run
 * read-only show commands through this window, and every call it makes is
 * listed here with exactly what it was given, already redacted.
 */
export function AiSettings() {
  const projectName = useStore((s) => s.meta?.name ?? 'project');
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [activity, setActivity] = useState<McpActivity[]>([]);
  const [copied, setCopied] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await ipc.mcpStatus());
      setActivity(await ipc.mcpActivity(false, 50));
    } catch (e) {
      setProblem(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    let stop: (() => void) | undefined;
    void ipc.onMcpActivity((e) => {
      if (!e.running) void ipc.mcpActivity(false, 50).then(setActivity).catch(() => {});
    }).then((s) => {
      stop = s;
    });
    return () => stop?.();
  }, [refresh]);

  const toggle = async (on: boolean) => {
    // The switch moves at once; the reply then says what is really so.
    const before = status;
    setStatus((s) => (s ? { ...s, enabled: on } : s));
    setBusy(true);
    setProblem(null);
    try {
      setStatus(await ipc.mcpSetEnabled(on));
    } catch (e) {
      setStatus(before);
      setProblem(String(e));
    } finally {
      setBusy(false);
    }
  };

  const copy = (which: string, text: string) => {
    void navigator.clipboard?.writeText(text).then(() => {
      setCopied(which);
      window.setTimeout(() => setCopied(null), 1500);
    });
  };

  const exportRecord = async () => {
    try {
      const md = await ipc.mcpChangeRecord();
      await saveExport(`${slug(projectName)}-ai-change-record.md`, md, 'text/markdown');
    } catch (e) {
      setProblem(String(e));
    }
  };

  const clear = async () => {
    try {
      await ipc.mcpClearActivity(false);
      setActivity([]);
    } catch (e) {
      setProblem(String(e));
    }
  };

  const state = !status
    ? null
    : status.error
      ? { tone: 'problem', text: status.error }
      : status.listening
        ? { tone: 'on', text: t('ai.listening') }
        : { tone: 'off', text: t('ai.off') };

  return (
    <section id="settings-ai" data-settings-section="ai" className="cv-settings-block" data-region="ai">
      <h2>{t('settings.ai')}</h2>
      <p className="cv-help">{t('ai.intro')}</p>
      {!isDesktop && <p className="cv-help">{t('ai.desktopOnly')}</p>}
      <label className="cv-check">
        <input type="checkbox" data-field="mcp-enabled" disabled={!status || busy} checked={!!status?.enabled} onChange={(e) => void toggle(e.target.checked)} />
        {t('ai.enable')}
      </label>
      {state && (
        <p className={state.tone === 'problem' ? 'cv-help cv-update-problem' : 'cv-help'} data-hint="mcp-state" data-tone={state.tone} role="status">
          {state.text}
        </p>
      )}
      <p className="cv-help">{t('ai.data')}</p>
      {status?.enabled && (
        <div className="cv-ai-setup" data-region="ai-setup">
          <h3>{t('ai.claudeCode')}</h3>
          <p className="cv-help">{t('ai.claudeCodeHint')}</p>
          <pre className="cv-mono" data-field="claude-code-command">{status.claudeCodeCommand}</pre>
          <button type="button" className="cv-btn cv-btn-small" data-action="copy-claude-code" onClick={() => copy('code', status.claudeCodeCommand)}>
            {copied === 'code' ? t('ai.copied') : t('ai.copy')}
          </button>
          <h3>{t('ai.claudeDesktop')}</h3>
          <p className="cv-help">{t('ai.claudeDesktopHint')}</p>
          <pre className="cv-mono" data-field="claude-desktop-config">{status.claudeDesktopConfig}</pre>
          <button type="button" className="cv-btn cv-btn-small" data-action="copy-claude-desktop" onClick={() => copy('desktop', status.claudeDesktopConfig)}>
            {copied === 'desktop' ? t('ai.copied') : t('ai.copy')}
          </button>
        </div>
      )}
      <h3>{t('ai.activity')}</h3>
      <p className="cv-help">{t('ai.activityHint')}</p>
      <div className="cv-row">
        <button type="button" className="cv-btn cv-btn-small" data-action="ai-refresh" onClick={() => void refresh()}>{t('ai.refresh')}</button>
        <button type="button" className="cv-btn cv-btn-small" data-action="ai-export" disabled={activity.length === 0} onClick={() => void exportRecord()}>{t('ai.export')}</button>
        <button type="button" className="cv-btn cv-btn-small" data-action="ai-clear" disabled={activity.length === 0} onClick={() => void clear()}>{t('ai.clear')}</button>
      </div>
      {activity.length === 0 ? (
        <p className="cv-help" data-hint="ai-no-activity">{t('ai.noActivity')}</p>
      ) : (
        <ul className="cv-ai-activity" data-region="ai-activity">
          {activity.map((a) => (
            <li key={a.id} data-outcome={a.outcome} data-tool={a.tool}>
              <details>
                <summary>
                  <span className="cv-mono">{new Date(a.atMs).toLocaleString()}</span>{' '}
                  <strong>{a.tool}</strong>
                  {a.device && <> {t('ai.on', { device: a.device })}</>}
                  {a.command && <> · <code>{a.command}</code></>} · {t(`ai.outcome.${a.outcome}`)}
                </summary>
                <pre className="cv-mono cv-ai-result">{a.result}</pre>
              </details>
            </li>
          ))}
        </ul>
      )}
      {problem && <p className="cv-help cv-update-problem" role="alert">{problem}</p>}
    </section>
  );
}

/**
 * The status strip's sign that Claude is using Corelens right now: the
 * tool and the device, for as long as the call runs. Nothing when idle.
 */
export function AiActivity() {
  const [calls, setCalls] = useState<{ key: number; tool: string; device: string }[]>([]);
  useEffect(() => {
    let stop: (() => void) | undefined;
    let next = 0;
    void ipc.onMcpActivity((e) => {
      setCalls((now) => {
        if (e.running) return [...now, { key: next++, tool: e.tool, device: e.device }];
        const i = now.findIndex((c) => c.tool === e.tool);
        return i < 0 ? now : [...now.slice(0, i), ...now.slice(i + 1)];
      });
    }).then((s) => {
      stop = s;
    });
    return () => stop?.();
  }, []);
  const c = calls.at(-1);
  if (!c) return null;
  return (
    <span className="cv-ai-busy" role="status" aria-live="polite" data-region="ai-busy">
      {c.device ? t('ai.busyOn', { tool: c.tool, device: c.device }) : t('ai.busy', { tool: c.tool })}
    </span>
  );
}
