import { useCallback, useEffect, useRef, useState } from 'react';

import { t } from '../i18n';
import { applyEvent, isChatShortcut, itemsFromHistory, tokens, type ChatItem, type Table } from '../lib/aiChat';
import { ipc, isDesktop, type AiSettingsDto, type AiUsage } from '../lib/ipc';
import { useStore } from '../state/store';
import { ChromeIcon, IconButton } from './chromeIcons';

const DOCK_KEY = 'corelens.ai.chatDocked';

function remembered(): boolean {
  try {
    return localStorage.getItem(DOCK_KEY) === '1';
  } catch {
    return false;
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
  const [docked, setDocked] = useState(remembered);
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

  const dock = (on: boolean) => {
    setDocked(on);
    try {
      localStorage.setItem(DOCK_KEY, on ? '1' : '0');
    } catch {
      // A per-viewer convenience; without storage it is simply not kept.
    }
  };

  const send = async () => {
    const q = text.trim();
    if (!q || running) return;
    setProblem(null);
    setItems((list) => [...list, { kind: 'user', text: context ? `[About: ${context}]\n${q}` : q }]);
    setText('');
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
          <IconButton icon={docked ? 'pop-out' : 'pin'} label={docked ? t('aiChat.float') : t('aiChat.dock')} onClick={() => dock(!docked)} region="ai-chat-dock" />
          <IconButton icon="hide" label={t('aiChat.collapse')} shortcut="Ctrl+Shift+A" onClick={() => setOpen(false)} region="ai-chat-collapse" />
        </span>
      </header>
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
