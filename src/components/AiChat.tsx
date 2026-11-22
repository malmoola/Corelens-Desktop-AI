import { useCallback, useEffect, useRef, useState } from 'react';

import { t } from '../i18n';
import { applyEvent, isChatShortcut, itemsFromHistory, tokens, type ChatItem, type Table } from '../lib/aiChat';
import { ipc, isDesktop, type AiSettingsDto, type AiUsage, type LibraryStatusDto } from '../lib/ipc';
import { useStore } from '../state/store';
import { ChromeIcon, IconButton } from './chromeIcons';

const DOCK_KEY = 'corelens.ai.chatDocked';
const RUNBOOKS_KEY = 'corelens.ai.runbooksOpen';

function remembered(key = DOCK_KEY): boolean {
  try {
    return localStorage.getItem(key) === '1';
  } catch {
    return false;
  }
}

function remember(key: string, on: boolean) {
  try {
    localStorage.setItem(key, on ? '1' : '0');
  } catch {
    // A per-viewer convenience; without storage it is simply not kept.
  }
}

/**
 * Ask AI: a floating button, bottom right, and the chat it opens.
 *
 * The conversation belongs to the project open in the window and is kept
 * by Rust; this panel draws it, sends the engineer's message and Stop, and
 * draws each turn as its events arrive. It never holds a key and never
 * talks to a provider: Rust does, from the tool layer MCP uses.
 */
export function AiChat() {
  const open = useStore((s) => s.aiChatOpen);
  const setOpen = useStore((s) => s.setAiChatOpen);
  const context = useStore((s) => s.aiChatContext);
  const projectId = useStore((s) => s.meta?.id ?? null);
  const changes = useStore((s) => s.aiChanges);
  const openApproval = useStore((s) => s.openApproval);
  const [items, setItems] = useState<ChatItem[]>([]);
  const [session, setSession] = useState<AiUsage>({ inputTokens: 0, outputTokens: 0 });
  const [turn, setTurn] = useState<AiUsage | null>(null);
  const [running, setRunning] = useState(false);
  const [text, setText] = useState('');
  const [settings, setSettings] = useState<AiSettingsDto | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [docked, setDocked] = useState(() => remembered());
  const [showRunbooks, setShowRunbooks] = useState(() => remembered(RUNBOOKS_KEY));
  const [library, setLibrary] = useState<LibraryStatusDto | null>(null);
  const body = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);

  const load = useCallback(async () => {
    try {
      const h = await ipc.aiChatHistory();
      setItems(itemsFromHistory(h.rows));
      setSession(h.session);
      setRunning(h.running);
      setSettings(await ipc.aiSettings());
    } catch (e) {
      setProblem(String(e));
    }
  }, []);

  // The runbooks and how far each Part got: read when the list is shown,
  // and again when a turn ends, since the chat records steps as it goes.
  const loadLibrary = useCallback(async () => {
    try {
      setLibrary(await ipc.libraryStatus());
    } catch (e) {
      setProblem(String(e));
    }
  }, []);
  useEffect(() => {
    if (open && showRunbooks) void loadLibrary();
  }, [open, showRunbooks, projectId, running, loadLibrary]);

  useEffect(() => {
    if (open) void load();
  }, [open, projectId, load]);

  useEffect(() => {
    // A listener that arrives after the effect was cleaned up (React runs
    // it twice in development) removes itself, or every event is drawn
    // twice.
    let stop: (() => void) | undefined;
    let gone = false;
    void ipc.onAiChat((e) => {
      if (e.projectId !== useStore.getState().meta?.id) return;
      if (e.kind === 'started') {
        setRunning(true);
        setTurn(null);
      } else if (e.kind === 'done') {
        setRunning(false);
        setTurn(e.usage);
        setSession(e.session);
      } else if (e.kind === 'error' || e.kind === 'stopped') {
        setRunning(false);
      }
      setItems((list) => (e.kind === 'stopped' ? [...list, { kind: 'notice', text: t('aiChat.stopped'), tone: 'info' }] : applyEvent(list, e)));
    }).then((s) => {
      if (gone) s();
      else stop = s;
    });
    return () => {
      gone = true;
      stop?.();
    };
  }, []);

  // Ctrl+Shift+A opens and closes it, wherever the focus is.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!isChatShortcut(e)) return;
      e.preventDefault();
      setOpen(!useStore.getState().aiChatOpen);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [setOpen]);

  useEffect(() => {
    body.current?.scrollTo({ top: body.current.scrollHeight });
  }, [items]);

  useEffect(() => {
    if (open) input.current?.focus();
  }, [open, context]);

  // A question another panel wrote for the engineer to send (or edit).
  const draft = useStore((s) => s.aiChatDraft);
  useEffect(() => {
    if (draft === null) return;
    setText(draft);
    useStore.setState({ aiChatDraft: null });
  }, [draft]);

  const dock = (on: boolean) => {
    setDocked(on);
    remember(DOCK_KEY, on);
  };

  const toggleRunbooks = () => {
    setShowRunbooks(!showRunbooks);
    remember(RUNBOOKS_KEY, !showRunbooks);
  };

  const send = async (asked?: string) => {
    const q = (asked ?? text).trim();
    if (!q || running) return;
    setProblem(null);
    setItems((list) => [...list, { kind: 'user', text: context ? `[About: ${context}]\n${q}` : q }]);
    if (asked === undefined) setText('');
    try {
      await ipc.aiChatSend(q, context);
      useStore.setState({ aiChatContext: null });
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
      setRunning(false);
    }
  };

  const stopTurn = async () => {
    try {
      await ipc.aiChatStop();
    } catch (e) {
      setProblem(String(e));
    }
    setRunning(false);
  };

  if (!projectId) return null;
  if (!open) {
    return (
      <button type="button" className="cv-ask-ai" data-action="ask-ai" title={t('aiChat.open')} onClick={() => setOpen(true)}>
        <ChromeIcon name="chat" />
        <span>{t('aiChat.button')}</span>
      </button>
    );
  }
  const off = settings !== null && !settings.chatEnabled;
  const model = settings ? `${settings.provider} · ${settings.models[settings.provider]}` : '';
  return (
    <section className={`cv-ai-chat${docked ? ' is-docked' : ''}`} data-region="ai-chat" aria-label={t('aiChat.title')}>
      <header className="cv-ai-chat-head">
        <strong>{t('aiChat.title')}</strong>
        <span className="cv-help cv-ai-chat-model" data-hint="ai-chat-model">{model}</span>
        <span className="cv-help" data-hint="ai-chat-tokens" title={t('aiChat.tokensHint')}>
          {turn ? t('aiChat.turnTokens', { input: tokens(turn.inputTokens), output: tokens(turn.outputTokens) }) + ' · ' : ''}
          {t('aiChat.sessionTokens', { input: tokens(session.inputTokens), output: tokens(session.outputTokens) })}
        </span>
        <span className="cv-ai-chat-tools">
          <IconButton icon="page" label={t('runbooks.toggle')} pressed={showRunbooks} onClick={toggleRunbooks} region="ai-chat-runbooks" />
          <IconButton icon={docked ? 'pop-out' : 'pin'} label={docked ? t('aiChat.float') : t('aiChat.dock')} onClick={() => dock(!docked)} region="ai-chat-dock" />
          <IconButton icon="hide" label={t('aiChat.collapse')} shortcut="Ctrl+Shift+A" onClick={() => setOpen(false)} region="ai-chat-collapse" />
        </span>
      </header>
      {showRunbooks && (
        <Runbooks
          library={library}
          busy={running || off}
          onStart={(q) => void send(q)}
          onReset={async (doc, part) => {
            try {
              setLibrary(await ipc.runbookStepSet(doc, part, null, 'pending'));
            } catch (e) {
              setProblem(String(e));
            }
          }}
        />
      )}
      <div className="cv-ai-chat-body" ref={body} data-region="ai-chat-body" aria-live="polite">
        {off && <p className="cv-help" data-hint="ai-chat-off">{t('aiChat.off')}</p>}
        {!off && items.length === 0 && <p className="cv-help">{t('aiChat.empty')}</p>}
        {items.map((item, i) => (
          <Item key={i} item={item} changes={changes} onOpenApproval={openApproval} />
        ))}
        {running && <p className="cv-help cv-ai-chat-running" data-hint="ai-chat-running">{t('aiChat.thinking')}</p>}
      </div>
      {problem && <p className="cv-help cv-update-problem" role="alert" data-hint="ai-chat-problem">{problem}</p>}
      <footer className="cv-ai-chat-foot">
        {context && (
          <span className="cv-ai-chat-context" data-field="ai-chat-context">
            {t('aiChat.about', { what: context })}
            <IconButton icon="x" label={t('aiChat.dropContext')} onClick={() => useStore.setState({ aiChatContext: null })} />
          </span>
        )}
        <textarea
          ref={input}
          className="cv-input cv-ai-chat-input"
          data-field="ai-chat-input"
          rows={3}
          value={text}
          disabled={off || !isDesktop}
          placeholder={t('aiChat.placeholder')}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
        />
        {running ? (
          <button type="button" className="cv-btn cv-btn-stop" data-action="ai-chat-stop" onClick={() => void stopTurn()}>
            {t('aiChat.stop')}
          </button>
        ) : (
          <button type="button" className="cv-btn cv-btn-primary" data-action="ai-chat-send" disabled={off || !text.trim()} onClick={() => void send()}>
            {t('aiChat.send')}
          </button>
        )}
      </footer>
    </section>
  );
}

/** The project's runbooks, each Part with how far it got and a button
 *  that asks the chat to start or continue it. */
function Runbooks({ library, busy, onStart, onReset }: { library: LibraryStatusDto | null; busy: boolean; onStart: (q: string) => void; onReset: (doc: string, part: string) => void }) {
  if (!library) return <div className="cv-ai-runbooks" data-region="ai-runbooks" />;
  const books = library.documents.filter((d) => d.runbook);
  const noFolder = !library.folders.project && !library.folders.library;
  return (
    <div className="cv-ai-runbooks" data-region="ai-runbooks">
      {noFolder && <p className="cv-help" data-hint="runbooks-no-folder">{t('runbooks.noFolder')}</p>}
      {!noFolder && books.length === 0 && <p className="cv-help" data-hint="runbooks-none">{t('runbooks.none')}</p>}
      {books.map((d) => (
        <div key={d.name} className="cv-ai-runbook" data-runbook={d.name}>
          <div className="cv-ai-runbook-title">
            <strong>{d.runbook}</strong> <span className="cv-help">{d.name}</span>
          </div>
          <ul>
            {d.parts.map((p) => {
              const started = p.done > 0 || p.failed > 0;
              const finished = p.next === null;
              return (
                <li key={p.label} data-part={p.label} className={finished ? 'is-done' : p.failed ? 'is-failed' : ''}>
                  <span className="cv-ai-runbook-part" title={p.title}>{p.label}</span>
                  <span className="cv-help" data-hint="runbook-progress">
                    {t('runbooks.progress', { done: p.done, steps: p.steps })}
                    {p.failed > 0 && <> · {t('runbooks.failed', { count: p.failed })}</>}
                  </span>
                  {!finished && (
                    <button
                      type="button"
                      className="cv-btn cv-btn-small"
                      data-action={started ? 'runbook-continue' : 'runbook-start'}
                      disabled={busy}
                      onClick={() => onStart(started && p.next ? t('runbooks.continueAsk', { part: p.label, doc: d.name, step: p.next }) : t('runbooks.startAsk', { part: p.label, doc: d.name }))}
                    >
                      {started ? t('runbooks.continue') : t('runbooks.start')}
                    </button>
                  )}
                  {started && (
                    <button type="button" className="cv-btn cv-btn-small" data-action="runbook-reset" disabled={busy} onClick={() => onReset(d.name, p.label)}>
                      {t('runbooks.again')}
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      ))}
      {books.length > 0 && <p className="cv-help">{t('runbooks.hint')}</p>}
    </div>
  );
}

function TableView({ table }: { table: Table }) {
  return (
    <div className="cv-ai-chat-table">
      <table>
        <thead>
          <tr>
            {table.columns.map((c) => (
              <th key={c}>{c}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {table.rows.slice(0, 200).map((r, i) => (
            <tr key={i}>
              {r.map((c, j) => (
                <td key={j}>{c}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Item({ item, changes, onOpenApproval }: { item: ChatItem; changes: ReturnType<typeof useStore.getState>['aiChanges']; onOpenApproval: (id: string | null) => void }) {
  if (item.kind === 'user') return <div className="cv-ai-msg is-user" data-kind="user">{item.text}</div>;
  if (item.kind === 'assistant') return <div className="cv-ai-msg is-assistant" data-kind="assistant">{item.text}</div>;
  if (item.kind === 'notice') return <div className={`cv-ai-msg is-notice is-${item.tone}`} data-kind="notice">{item.text}</div>;
  const change = item.changeId ? changes[item.changeId] : undefined;
  const isProposal = item.name === 'propose_config_change' || item.name === 'propose_save_config';
  return (
    <div className={`cv-ai-tool${item.isError ? ' is-error' : ''}`} data-kind="tool" data-tool={item.name}>
      <details open={isProposal}>
        <summary>
          <ChromeIcon name={item.result === null ? 'more' : item.isError ? 'alert' : 'check'} /> <code>{item.name}</code>
          {item.target && <> {t('ai.on', { device: item.target })}</>}
          {item.command && <> · <code>{item.command}</code></>}
        </summary>
        {item.table && <TableView table={item.table} />}
        {item.result !== null && <pre className="cv-mono cv-ai-result">{item.result}</pre>}
      </details>
      {isProposal && item.result === null && <p className="cv-help" data-hint="ai-chat-awaiting">{t('aiChat.awaitingApproval')}</p>}
      {isProposal && (item.changeId || item.result === null) && (
        <button
          type="button"
          className="cv-btn cv-btn-small"
          data-action="ai-chat-open-approval"
          onClick={() => {
            const pending = Object.values(useStore.getState().aiChanges).find((c) => c.status === 'pending' && c.source === 'chat');
            onOpenApproval(item.changeId ?? pending?.id ?? null);
          }}
        >
          {t('aiChat.openApproval')}
          {change ? ` · ${t(`changes.status.${change.status}`)}` : ''}
        </button>
      )}
    </div>
  );
}
