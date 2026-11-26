import { useCallback, useEffect, useState } from 'react';

import { t } from '../i18n';
import { ipc, isDesktop, type AiModel, type AiProviderId, type AiSettingsDto, type AppLogInfo, type Change, type LibraryStatusDto, type McpActivity, type McpClientId, type McpStatus } from '../lib/ipc';
import { saveExport, slug } from '../lib/exports';
import { canRollBack } from '../lib/changes';
import { modelChoices } from '../lib/aiModels';
import { useStore } from '../state/store';

/**
 * AI's settings, in two places. The application's (`app`, on the start
 * screen): the chat on or off, the providers and models, the library
 * folder every project shares, AI assistants through MCP,
 * how long records are kept and the log. A project's (`project`, inside
 * it): its own documents folder, what assistants asked in it, its change
 * record, and clearing its chat.
 */
export function AiSettings({ scope }: { scope: 'app' | 'project' }) {
  // One copy of the settings and of the models each provider listed, so the
  // chat's model and the provider's model are the same choice wherever it
  // is made.
  const shared = useAiSettings();
  const [listed, setListed] = useState<Partial<Record<AiProviderId, AiModel[]>>>({});
  if (scope === 'project') {
    return (
      <section id="settings-aiproject" data-settings-section="aiproject" className="cv-settings-block" data-region="ai-project">
        <h2>{t('settings.aiProject')}</h2>
        <DocumentsSection scope="project" />
        <McpActivitySection />
        <ChangeHistory />
        <ProjectChatClear />
      </section>
    );
  }
  return (
    <section id="settings-ai" data-settings-section="ai" className="cv-settings-block" data-region="ai">
      <h2>{t('settings.ai')}</h2>
      <ChatSection ai={shared} listed={listed} />
      <ProvidersSection ai={shared} listed={listed} setListed={setListed} />
      <DocumentsSection scope="library" />
      <McpSection />
      <RetentionSection />
    </section>
  );
}

const PROVIDER_NAMES: Record<AiProviderId, string> = { anthropic: 'Anthropic', openai: 'OpenAI', ollama: 'Ollama', snowflake: 'Snowflake Cortex' };

/** Models offered before a list is fetched — fetching is a call to the
 *  provider, made only when Refresh models is pressed. */
const SUGGESTED: Record<AiProviderId, string[]> = {
  anthropic: ['claude-opus-5-5', 'claude-sonnet-5-5', 'claude-haiku-5-5', 'claude-fable-5-2'],
  openai: [],
  ollama: ['qwen3', 'llama3.1', 'mistral-small3.1', 'gpt-oss'],
  snowflake: ['claude-sonnet-5-5', 'claude-opus-5-5', 'claude-haiku-5-5', 'claude-sonnet-4-5', 'openai-gpt-5.4', 'openai-gpt-5'],
};

type Ai = ReturnType<typeof useAiSettings>;

const OTHER = '\u0000other';

function ModelPicker({ provider, chosen, listed, onChoose, field }: { provider: AiProviderId; chosen: string; listed: readonly AiModel[]; onChoose: (id: string) => void; field: string }) {
  const [typing, setTyping] = useState(false);
  const choices = modelChoices(chosen, listed, SUGGESTED[provider]);
  if (typing) {
    return (
      <input
        className="cv-input"
        data-field={field}
        autoFocus
        defaultValue={chosen}
        placeholder={t('aiSettings.modelTyped')}
        onBlur={(e) => {
          setTyping(false);
          if (e.target.value.trim() && e.target.value.trim() !== chosen) onChoose(e.target.value.trim());
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
          if (e.key === 'Escape') setTyping(false);
        }}
      />
    );
  }
  return (
    <select
      className="cv-input"
      data-field={field}
      value={chosen}
      onChange={(e) => {
        if (e.target.value === OTHER) setTyping(true);
        else onChoose(e.target.value);
      }}
    >
      {choices.map((m) => (
        <option key={m.id} value={m.id}>
          {m.id}
          {m.label ? ` (${m.label})` : ''}
          {m.tools === false ? ` — ${t('aiSettings.noTools')}` : ''}
        </option>
      ))}
      <option value={OTHER}>{t('aiSettings.modelOther')}</option>
    </select>
  );
}

function useAiSettings() {
  const [settings, setSettings] = useState<AiSettingsDto | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    void ipc.aiSettings().then(setSettings).catch((e: unknown) => setProblem(String(e)));
  }, []);
  const set = async (key: string, value: string | null) => {
    setProblem(null);
    try {
      setSettings(await ipc.aiSettingSet(key, value));
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    }
  };
  return { settings, setSettings, set, problem, setProblem };
}

/** The folders of the engineer's own documents the chat searches first:
 *  the project's, and the library every project shares. Rust reads them;
 *  this lists what it found. */
/** The documents folder of one scope, and what was read from it. */
function DocumentsSection({ scope }: { scope: 'library' | 'project' }) {
  const projectId = useStore((s) => s.meta?.id ?? null);
  const [status, setStatus] = useState<LibraryStatusDto | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(async () => {
    setBusy(true);
    try {
      setStatus(await ipc.libraryStatus());
      setProblem(null);
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);
  useEffect(() => {
    void refresh();
  }, [refresh, projectId]);
  const choose = async () => {
    const current = (scope === 'library' ? status?.folders.library : status?.folders.project) ?? undefined;
    const picked = await ipc.pickFolder(t('docs.chooseTitle'), current);
    if (!picked) return;
    setBusy(true);
    try {
      setStatus(await ipc.libraryFolderSet(scope, picked));
      setProblem(null);
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const clear = async () => {
    try {
      setStatus(await ipc.libraryFolderSet(scope, null));
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    }
  };
  const row = (label: string, hint: string, path: string | null | undefined, disabled: boolean) => (
    <div className="cv-docs-folder" data-region={`docs-folder-${scope}`}>
      <span className="cv-docs-folder-label">
        <strong>{label}</strong>
        <span className="cv-help">{disabled ? t('docs.noProject') : hint}</span>
      </span>
      <code className="cv-docs-path" data-field={`docs-path-${scope}`}>{path ?? t('docs.notSet')}</code>
      <span className="cv-row">
        <button type="button" className="cv-btn cv-btn-small" data-action={`docs-choose-${scope}`} disabled={disabled || busy || !isDesktop} onClick={() => void choose()}>{t('docs.choose')}</button>
        <button type="button" className="cv-btn cv-btn-small" data-action={`docs-clear-${scope}`} disabled={disabled || !path} onClick={() => void clear()}>{t('docs.clear')}</button>
      </span>
    </div>
  );
  const docs = (status?.documents ?? []).filter((d) => d.project === (scope === 'project'));
  const folder = scope === 'project' ? status?.folders.project : status?.folders.library;
  return (
    <div data-region="ai-documents">
      <h3>{t('docs.title')}</h3>
      <p className="cv-help">{t('docs.intro')}</p>
      {scope === 'project'
        ? row(t('docs.project'), t('docs.projectHint'), folder, !projectId)
        : row(t('docs.library'), t('docs.libraryHint'), folder, false)}
      {problem && <p className="cv-help cv-update-problem" role="alert">{problem}</p>}
      {folder && (
        <>
          <p className="cv-help cv-row" data-hint="docs-count">
            {docs.length === 0 ? t('docs.none') : t('docs.count', { count: docs.length })}
            <button type="button" className="cv-btn cv-btn-small" data-action="docs-refresh" disabled={busy} onClick={() => void refresh()}>{t('docs.refresh')}</button>
          </p>
          {docs.length > 0 && (
            <ul className="cv-docs-list" data-region="docs-list">
              {docs.map((d) => (
                <li key={`${d.project ? 'p' : 'l'}:${d.name}`} data-doc={d.name} data-kind={d.runbook ? 'runbook' : d.kind}>
                  <code>{d.name}</code>
                  <span className="cv-help"> · {t(d.project ? 'docs.whose.project' : 'docs.whose.library')}</span>
                  {d.runbook && <span className="cv-docs-badge"> {t('docs.runbook', { count: d.parts.length })}</span>}
                  {d.copyOf && <span className="cv-help"> · {t('docs.copyOf', { name: d.copyOf })}</span>}
                  {d.error && <span className="cv-help cv-update-problem"> · {t('docs.unreadable', { why: d.error })}</span>}
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}

/** 1. The chat inside Corelens: on or off, which provider, which model. */
function ChatSection({ ai, listed }: { ai: Ai; listed: Partial<Record<AiProviderId, AiModel[]>> }) {
  const { settings, set, problem, setProblem } = ai;
  const [result, setResult] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  if (!settings) return <div data-region="ai-chat-settings">{!isDesktop && <p className="cv-help">{t('ai.desktopOnly')}</p>}</div>;
  const p = settings.provider;
  const test = async () => {
    setTesting(true);
    setResult(null);
    setProblem(null);
    try {
      setResult(await ipc.aiTestConnection(p));
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    } finally {
      setTesting(false);
    }
  };
  return (
    <div className="cv-ai-part" data-region="ai-chat-settings">
      <h3>{t('aiSettings.chatTitle')}</h3>
      <p className="cv-help">{t('aiSettings.chatIntro')}</p>
      <label className="cv-check">
        <input type="checkbox" data-field="ai-chat-enabled" checked={settings.chatEnabled} onChange={(e) => void set('aiChatEnabled', e.target.checked ? '1' : null)} />
        {t('aiSettings.chatEnable')}
      </label>
      <div className="cv-row">
        <label className="cv-field">
          <span>{t('aiSettings.provider')}</span>
          <select className="cv-input" data-field="ai-provider" value={p} onChange={(e) => void set('aiProvider', e.target.value)}>
            {(Object.keys(PROVIDER_NAMES) as AiProviderId[]).map((id) => (
              <option key={id} value={id}>{PROVIDER_NAMES[id]}</option>
            ))}
          </select>
        </label>
        <label className="cv-field">
          <span>{t('aiSettings.model')}</span>
          <ModelPicker provider={p} chosen={settings.models[p] ?? ''} listed={listed[p] ?? []} field="ai-chat-model" onChoose={(id) => void set(`aiModel.${p}`, id)} />
        </label>
        <button type="button" className="cv-btn" data-action="ai-test-connection" disabled={!settings.chatEnabled || testing} onClick={() => void test()}>
          {testing ? t('aiSettings.testing') : t('aiSettings.test')}
        </button>
      </div>
      {!settings.chatEnabled && <p className="cv-help" data-hint="ai-chat-off-note">{t('aiSettings.offNote')}</p>}
      {result && <p className="cv-help" role="status" data-hint="ai-test-result">{result}</p>}
      {problem && <p className="cv-help cv-update-problem" role="alert">{problem}</p>}
      <p className="cv-help">{t('aiSettings.data')}</p>
    </div>
  );
}

/** 2. Each provider: its key (kept in the system keychain), its address,
 *  its models. */
function ProvidersSection({ ai, listed: models, setListed: setModels }: { ai: Ai; listed: Partial<Record<AiProviderId, AiModel[]>>; setListed: (m: Partial<Record<AiProviderId, AiModel[]>>) => void }) {
  const { settings, setSettings, set, problem, setProblem } = ai;
  const [lastRequest, setLastRequest] = useState<{ provider: AiProviderId; body: string } | null>(null);
  // One provider at a time; until one is chosen here, the chat's.
  const [picked, setPicked] = useState<AiProviderId | null>(null);
  if (!settings) return null;
  const shown = picked ?? settings.provider;
  const optionLabel = (p: AiProviderId) =>
    p === 'ollama'
      ? t('aiSettings.optLocal', { name: PROVIDER_NAMES[p] })
      : t(settings.keys[p] ? 'aiSettings.optKeyKept' : 'aiSettings.optNoKey', { name: PROVIDER_NAMES[p] });
  const run = async (f: () => Promise<void>) => {
    setProblem(null);
    try {
      await f();
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    }
  };
  const refresh = (p: AiProviderId) => run(async () => setModels({ ...models, [p]: await ipc.aiModels(p) }));
  const card = (p: AiProviderId) => {
    const list = models[p] ?? [];
    const chosen = settings.models[p] ?? '';
    const known = list.find((m) => m.id === chosen);
    return (
      <div className="cv-ai-provider" key={p} data-provider={p}>
        {p !== 'ollama' && (
          <div className="cv-row">
            <span className="cv-help" data-hint={`ai-key-${p}`}>{settings.keys[p] ? t('aiSettings.keyKept') : t('aiSettings.keyNone')}</span>
            <button type="button" className="cv-btn cv-btn-small" data-action={`ai-key-import-${p}`} onClick={() => void run(async () => setSettings(await ipc.aiKeyImport(p)))}>
              {t('aiSettings.pasteKey')}
            </button>
            {settings.keys[p] && (
              <button type="button" className="cv-btn cv-btn-small is-danger" data-action={`ai-key-forget-${p}`} onClick={() => void run(async () => setSettings(await ipc.aiKeyForget(p)))}>
                {t('aiSettings.forgetKey')}
              </button>
            )}
          </div>
        )}
        {p !== 'ollama' && <p className="cv-help">{t(p === 'snowflake' ? 'aiSettings.snowflakeKeyHint' : 'aiSettings.keyHint')}</p>}
        {p === 'snowflake' && (
          <label className="cv-field">
            <span>{t('aiSettings.snowflakeAccount')}</span>
            <input className="cv-input" data-field="ai-snowflake-account" placeholder="orgname-accountname" defaultValue={settings.snowflakeAccount ?? ''} onBlur={(e) => void set('aiSnowflakeAccount', e.target.value)} />
          </label>
        )}
        {p === 'snowflake' && <p className="cv-help">{t('aiSettings.snowflakeHint')}</p>}
        {p === 'snowflake' && (
          <label className="cv-field">
            <span>{t('aiSettings.snowflakeLibrary')}</span>
            <input className="cv-input" data-field="ai-snowflake-library" placeholder="DATABASE.SCHEMA.SERVICE" defaultValue={settings.snowflakeLibrary ?? ''} onBlur={(e) => void set('aiSnowflakeLibrary', e.target.value)} />
          </label>
        )}
        {p === 'snowflake' && <p className="cv-help">{t('aiSettings.snowflakeLibraryHint')}</p>}
        {p === 'openai' && (
          <div className="cv-row">
            <label className="cv-field">
              <span>{t('aiSettings.baseUrl')}</span>
              <input className="cv-input" data-field="ai-openai-base" defaultValue={settings.openaiBaseUrl} onBlur={(e) => void set('aiOpenAiBaseUrl', e.target.value)} />
            </label>
            <label className="cv-field">
              <span>{t('aiSettings.apiStyle')}</span>
              <select className="cv-input" data-field="ai-openai-style" value={settings.openaiStyle} onChange={(e) => void set('aiOpenAiStyle', e.target.value === 'chat' ? 'chat' : null)}>
                <option value="responses">{t('aiSettings.styleResponses')}</option>
                <option value="chat">{t('aiSettings.styleChat')}</option>
              </select>
            </label>
          </div>
        )}
        {p === 'ollama' && (
          <label className="cv-field">
            <span>{t('aiSettings.baseUrl')}</span>
            <input className="cv-input" data-field="ai-ollama-base" defaultValue={settings.ollamaBaseUrl} onBlur={(e) => void set('aiOllamaBaseUrl', e.target.value)} />
          </label>
        )}
        <div className="cv-row">
          <label className="cv-field">
            <span>{t('aiSettings.model')}</span>
            <ModelPicker provider={p} chosen={chosen} listed={list} field={`ai-model-${p}`} onChoose={(id) => void set(`aiModel.${p}`, id)} />
          </label>
          <button type="button" className="cv-btn cv-btn-small" data-action={`ai-models-${p}`} disabled={!settings.chatEnabled} onClick={() => void refresh(p)}>
            {t('aiSettings.refreshModels')}
          </button>
          <button type="button" className="cv-btn cv-btn-small" data-action={`ai-last-request-${p}`} onClick={() => void run(async () => setLastRequest({ provider: p, body: await ipc.aiLastRequest(p) }))}>
            {t('aiSettings.lastRequest')}
          </button>
        </div>
        {known?.tools === false && <p className="cv-help cv-update-problem">{t('aiSettings.modelNoTools', { model: chosen })}</p>}
        {list.length > 0 && <p className="cv-help">{t('aiSettings.modelsListed', { n: list.length })}</p>}
        {lastRequest?.provider === p && (
          <details open className="cv-ai-last-request">
            <summary>{t('aiSettings.lastRequestTitle')}</summary>
            <p className="cv-help">{t('aiSettings.lastRequestHint')}</p>
            <pre className="cv-mono cv-ai-result" data-field={`ai-last-request-body-${p}`}>{lastRequest.body}</pre>
          </details>
        )}
      </div>
    );
  };
  return (
    <div className="cv-ai-part" data-region="ai-providers">
      <h3>{t('aiSettings.providersTitle')}</h3>
      {!settings.chatEnabled && <p className="cv-help">{t('aiSettings.providersOff')}</p>}
      <label className="cv-field">
        <span>{t('aiSettings.providerShown')}</span>
        <select className="cv-input" data-field="ai-provider-shown" value={shown} onChange={(e) => setPicked(e.target.value as AiProviderId)}>
          {(Object.keys(PROVIDER_NAMES) as AiProviderId[]).map((id) => (
            <option key={id} value={id}>{optionLabel(id)}</option>
          ))}
        </select>
      </label>
      {card(shown)}
      {problem && <p className="cv-help cv-update-problem" role="alert" data-hint="ai-providers-problem">{problem}</p>}
    </div>
  );
}

/** 4. How long the record, the chat and proposals are kept, and the log. */
function RetentionSection() {
  const { settings, set, problem, setProblem } = useAiSettings();
  const [log, setLog] = useState<AppLogInfo | null>(null);
  const [verbose, setVerbose] = useState(false);
  const [cleared, setCleared] = useState<number | null>(null);
  useEffect(() => {
    void ipc.appLogInfo().then((l) => {
      setLog(l);
      setVerbose(!!l && l.verboseUntilMs > Date.now());
    }).catch(() => undefined);
  }, []);
  if (!settings) return null;
  const days = (key: string, value: number, field: string) => (
    <select className="cv-input" data-field={field} value={value} onChange={(e) => void set(key, e.target.value)}>
      {[30, 90, 365].map((d) => (
        <option key={d} value={d}>{t('aiSettings.days', { n: d })}</option>
      ))}
    </select>
  );
  const verboseOn = verbose;
  const toggleVerbose = (on: boolean) => {
    // The box moves at once; Rust's answer then says until when.
    setVerbose(on);
    void ipc.appLogVerbose(on).then((l) => {
      setLog(l);
      setVerbose(l.verboseUntilMs > Date.now());
    }).catch((err: unknown) => {
      setVerbose(!on);
      setProblem(String(err));
    });
  };
  const clear = async () => {
    setProblem(null);
    try {
      setCleared(await ipc.aiChatClear(true));
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <div className="cv-ai-part" data-region="ai-retention">
      <h3>{t('aiSettings.retentionTitle')}</h3>
      <div className="cv-row">
        <label className="cv-field">
          <span>{t('aiSettings.auditRetention')}</span>
          {days('aiAuditRetentionDays', settings.auditRetentionDays, 'ai-audit-retention')}
        </label>
        <label className="cv-field">
          <span>{t('aiSettings.chatRetention')}</span>
          {days('aiChatRetentionDays', settings.chatRetentionDays, 'ai-chat-retention')}
        </label>
        <label className="cv-field">
          <span>{t('aiSettings.proposalTimeout')}</span>
          <select className="cv-input" data-field="ai-proposal-timeout" value={settings.proposalTimeoutMinutes} onChange={(e) => void set('aiProposalTimeoutMinutes', e.target.value)}>
            {[5, 10, 15, 30, 60].map((m) => (
              <option key={m} value={m}>{t('aiSettings.minutes', { n: m })}</option>
            ))}
          </select>
        </label>
      </div>
      <p className="cv-help">{t('aiSettings.retentionHint')}</p>
      <div className="cv-row">
        <button type="button" className="cv-btn cv-btn-small is-danger" data-action="ai-chat-clear-all" onClick={() => void clear()}>{t('aiSettings.clearAllChats')}</button>
        {cleared !== null && <span className="cv-help" role="status">{t('aiSettings.cleared', { n: cleared })}</span>}
      </div>
      <h4>{t('aiSettings.logTitle')}</h4>
      <label className="cv-check">
        <input type="checkbox" data-field="ai-verbose-logging" checked={verboseOn} onChange={(e) => toggleVerbose(e.target.checked)} />
        {t('aiSettings.verbose')}
      </label>
      <p className="cv-help">{verboseOn && log && log.verboseUntilMs > 0 ? t('aiSettings.verboseUntil', { when: new Date(log.verboseUntilMs).toLocaleString() }) : t('aiSettings.verboseHint')}</p>
      {log && (
        <p className="cv-help" data-hint="ai-log-size">
          {t('aiSettings.logSize', { size: (log.totalBytes / 1024 / 1024).toFixed(1), limit: Math.round(log.limitBytes / 1024 / 1024), folder: log.folder })}
        </p>
      )}
      {problem && <p className="cv-help cv-update-problem" role="alert">{problem}</p>}
    </div>
  );
}

/** The change record: every proposal, what was decided and what happened. */
function ChangeHistory() {
  const projectName = useStore((s) => s.meta?.name ?? 'project');
  const live = useStore((s) => s.aiChanges);
  const openApproval = useStore((s) => s.openApproval);
  const [list, setList] = useState<Change[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    void ipc.changeList(200).then(setList).catch((e: unknown) => setProblem(String(e)));
  }, [live]);
  const exportAs = async (format: 'md' | 'csv') => {
    try {
      const text = await ipc.changeExport(format);
      await saveExport(`${slug(projectName)}-config-changes.${format}`, text, format === 'md' ? 'text/markdown' : 'text/csv');
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <div className="cv-ai-part" data-region="ai-changes">
      <h3>{t('changes.historyTitle')}</h3>
      <p className="cv-help">{t('changes.historyHint')}</p>
      <div className="cv-row">
        <button type="button" className="cv-btn cv-btn-small" data-action="changes-export-md" disabled={list.length === 0} onClick={() => void exportAs('md')}>{t('changes.exportMd')}</button>
        <button type="button" className="cv-btn cv-btn-small" data-action="changes-export-csv" disabled={list.length === 0} onClick={() => void exportAs('csv')}>{t('changes.exportCsv')}</button>
      </div>
      {list.length === 0 ? (
        <p className="cv-help" data-hint="changes-none">{t('changes.none')}</p>
      ) : (
        <ul className="cv-ai-activity" data-region="changes-list">
          {list.map((c) => (
            <li key={c.id} data-change={c.id} data-status={c.status}>
              <button type="button" className="cv-link-btn" onClick={() => openApproval(c.id)}>
                <span className="cv-mono">{new Date(c.createdMs).toLocaleString()}</span> · {c.targets.map((x) => x.device).join(', ')} · {t(`changes.status.${c.status}`)}
                {c.targets.some((x) => canRollBack(x.status)) ? ` · ${t('changes.rollbackAvailable')}` : ''}
              </button>
              <span className="cv-help"> {c.reason}</span>
            </li>
          ))}
        </ul>
      )}
      {problem && <p className="cv-help cv-update-problem" role="alert">{problem}</p>}
    </div>
  );
}

/**
 * 3. Whether Claude (Claude Code or Claude Desktop, through MCP) may use
 * Corelens, how to connect it, and what it has been given.
 *
 * Off until switched on. While off nothing listens; while on, a client
 * started as `corelens-desktop-ai --mcp` can read the open project and run
 * read-only show commands through this window — and propose changes, which
 * only the approval dialog here can let through — and every call it makes
 * is listed here with exactly what it was given, already redacted.
 */
const CLIENT_KEY = 'corelens.ai.mcpClient';
function McpSection() {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [client, setClient] = useState<McpClientId>(() => {
    try {
      return (localStorage.getItem(CLIENT_KEY) as McpClientId | null) ?? 'claudeCode';
    } catch {
      return 'claudeCode';
    }
  });
  const chooseClient = (id: McpClientId) => {
    setClient(id);
    try {
      localStorage.setItem(CLIENT_KEY, id);
    } catch {
      // Remembering the choice is a convenience; the dropdown works without it.
    }
  };

  useEffect(() => {
    void ipc.mcpStatus().then(setStatus).catch((e: unknown) => setProblem(String(e)));
  }, []);

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

  // The one chosen, or the first if what was remembered is no longer offered.
  const setups = status?.setups ?? [];
  const setup = setups.find((x) => x.client === client) ?? setups[0];

  const state = !status
    ? null
    : status.error
      ? { tone: 'problem', text: status.error }
      : status.listening
        ? { tone: 'on', text: t('ai.listening') }
        : { tone: 'off', text: t('ai.off') };

  return (
    <div className="cv-ai-part" data-region="ai-mcp">
      <h3>{t('aiSettings.mcpTitle')}</h3>
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
      {status?.enabled && setup && (
        <div className="cv-ai-setup" data-region="ai-setup" data-client={setup.client}>
          <label className="cv-field">
            <span>{t('ai.client')}</span>
            <select className="cv-input" data-field="mcp-client" value={setup.client} onChange={(e) => chooseClient(e.target.value as McpClientId)}>
              {setups.map((x) => (
                <option key={x.client} value={x.client}>{t(`ai.client.${x.client}`)}</option>
              ))}
            </select>
          </label>
          <p className="cv-help" data-hint="mcp-client-hint">{t(`ai.clientHint.${setup.client}`)}</p>
          <pre className="cv-mono" data-field="mcp-setup-text">{setup.text}</pre>
          <button type="button" className="cv-btn cv-btn-small" data-action="copy-mcp-setup" onClick={() => copy(setup.client, setup.text)}>
            {copied === setup.client ? t('ai.copied') : t('ai.copy')}
          </button>
        </div>
      )}
      {problem && <p className="cv-help cv-update-problem" role="alert">{problem}</p>}
    </div>
  );
}

/** What Claude asked in the open project, with what Corelens returned. */
function McpActivitySection() {
  const projectName = useStore((s) => s.meta?.name ?? 'project');
  const [problem, setProblem] = useState<string | null>(null);
  const [activity, setActivity] = useState<McpActivity[]>([]);

  const refresh = useCallback(async () => {
    try {
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

  return (
    <div className="cv-ai-part" data-region="ai-mcp-activity">
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
    </div>
  );
}

/** The open project's chat, cleared on request. */
function ProjectChatClear() {
  const [cleared, setCleared] = useState<number | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const clear = async () => {
    setProblem(null);
    try {
      setCleared(await ipc.aiChatClear(false));
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <div className="cv-ai-part" data-region="ai-project-chat">
      <h3>{t('aiSettings.projectChatTitle')}</h3>
      <div className="cv-row">
        <button type="button" className="cv-btn cv-btn-small" data-action="ai-chat-clear" onClick={() => void clear()}>{t('aiSettings.clearChat')}</button>
        {cleared !== null && <span className="cv-help" role="status">{t('aiSettings.cleared', { n: cleared })}</span>}
      </div>
      {problem && <p className="cv-help cv-update-problem" role="alert">{problem}</p>}
    </div>
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
