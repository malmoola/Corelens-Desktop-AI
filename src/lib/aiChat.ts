/**
 * The chat panel's view of a conversation: the transcript Rust keeps, and
 * the events of a turn as it runs, made into the items the panel draws —
 * what the engineer asked, what the model said, each tool it called with
 * its target and command, the rows a template parsed, and the proposals it
 * made.
 */
import type { AiChatEvent, AiChatRow } from './ipc';

export type ChatItem =
  | { kind: 'user'; text: string }
  | { kind: 'assistant'; text: string }
  | {
      kind: 'tool';
      id: string;
      name: string;
      target: string;
      command: string;
      result: string | null;
      isError: boolean;
      table: Table | null;
      changeId: string | null;
    }
  | { kind: 'notice'; text: string; tone: 'info' | 'error' };

export interface Table {
  columns: string[];
  rows: string[][];
}

/** What a call names: its device and its command, as the panel shows them. */
export function describeCall(args: Record<string, unknown>): { target: string; command: string } {
  const str = (v: unknown) => (typeof v === 'string' ? v : '');
  const batch = Array.isArray(args.batch) ? (args.batch as Record<string, unknown>[]) : [];
  const target = str(args.target) || batch.map((b) => str(b.target)).filter(Boolean).join(', ');
  const commands = Array.isArray(args.commands) ? (args.commands as unknown[]).map(str).filter(Boolean).join(' / ') : '';
  const command = str(args.command) || commands || str(args.query);
  return { target, command };
}

/** The rows a template parsed, from `run_show_command`'s result: the line
 *  `Parsed with … (N row(s)):` and one JSON object a line after it. */
export function parseTable(result: string): Table | null {
  const lines = result.split('\n');
  const at = lines.findIndex((l) => /^Parsed with .+ \(\d+ row\(s\)\):$/.test(l.trim()));
  if (at < 0) return null;
  const objects: Record<string, unknown>[] = [];
  for (const line of lines.slice(at + 1)) {
    const t = line.trim();
    if (!t.startsWith('{')) break;
    try {
      const v = JSON.parse(t) as unknown;
      if (v && typeof v === 'object' && !Array.isArray(v)) objects.push(v as Record<string, unknown>);
    } catch {
      break;
    }
  }
  if (objects.length === 0) return null;
  const columns: string[] = [];
  for (const o of objects) for (const k of Object.keys(o)) if (!columns.includes(k)) columns.push(k);
  const cell = (v: unknown) => (v === null || v === undefined ? '' : Array.isArray(v) ? v.join(', ') : typeof v === 'object' ? JSON.stringify(v) : String(v));
  return { columns, rows: objects.map((o) => columns.map((c) => cell(o[c]))) };
}

/** The change a proposal's result names: `Change 1a2b3c4d (…`. */
export function changeIdOf(result: string): string | null {
  const m = /^Change ([0-9a-f]{8}) \(/m.exec(result);
  return m?.[1] ?? null;
}

function toolItem(id: string, name: string, args: Record<string, unknown>): ChatItem {
  const { target, command } = describeCall(args);
  return { kind: 'tool', id, name, target, command, result: null, isError: false, table: null, changeId: null };
}

function withResult(item: ChatItem, text: string, isError: boolean): ChatItem {
  if (item.kind !== 'tool') return item;
  return { ...item, result: text, isError, table: isError ? null : parseTable(text), changeId: changeIdOf(text) };
}

/** The transcript as items, each call with its result beside it. */
export function itemsFromHistory(rows: AiChatRow[]): ChatItem[] {
  const out: ChatItem[] = [];
  for (const row of rows) {
    const m = row.message;
    if (m.role === 'user') out.push({ kind: 'user', text: m.text });
    else if (m.role === 'assistant') {
      if (m.text.trim()) out.push({ kind: 'assistant', text: m.text });
      for (const c of m.calls) out.push(toolItem(c.id, c.name, c.arguments));
    } else {
      for (const r of m.results) {
        const i = out.findIndex((it) => it.kind === 'tool' && it.id === r.id && it.result === null);
        const found = out[i];
        if (found) out[i] = withResult(found, r.content, r.isError);
      }
    }
  }
  return out;
}

/** The items after one event of a running turn. Text arriving after a
 *  tool call starts a new answer below it. */
export function applyEvent(items: ChatItem[], e: AiChatEvent): ChatItem[] {
  switch (e.kind) {
    case 'text': {
      const last = items[items.length - 1];
      if (last && last.kind === 'assistant') return [...items.slice(0, -1), { kind: 'assistant', text: last.text + e.text }];
      return [...items, { kind: 'assistant', text: e.text }];
    }
    case 'toolCall':
      return [...items, { kind: 'tool', id: e.id, name: e.name, target: e.target, command: e.command, result: null, isError: false, table: null, changeId: null }];
    case 'toolResult': {
      const i = items.findIndex((it) => it.kind === 'tool' && it.id === e.id && it.result === null);
      const found = items[i];
      if (!found) return items;
      const next = items.slice();
      next[i] = withResult(found, e.text, e.isError);
      return next;
    }
    case 'notice':
      return [...items, { kind: 'notice', text: e.message, tone: 'info' }];
    case 'error':
      return [...items, { kind: 'notice', text: e.message, tone: 'error' }];
    default:
      return items;
  }
}

/** Tokens as the header shows them: `12.3k in · 1.2k out`. */
export function tokens(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 100_000 ? 0 : 1)}k` : String(n);
}

/** The keyboard shortcut that opens and closes the panel: Ctrl+Shift+A
 *  (Cmd+Shift+A on a Mac), never while typing in a field. */
export function isChatShortcut(e: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'shiftKey' | 'altKey'>): boolean {
  return (e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && e.key.toLowerCase() === 'a';
}

/** What a question about a device is about: its name and the address
 *  Corelens reaches it on, as the tools name it. */
export function deviceContext(data: { label?: string; hostname?: string; addresses?: { address?: string; isPrimary?: boolean }[] }): string {
  const name = (data.label || data.hostname || 'a device').trim();
  const addrs = data.addresses ?? [];
  const address = (addrs.find((a) => a.isPrimary) ?? addrs[0])?.address?.trim();
  return address ? `device ${name} (${address})` : `device ${name}`;
}

/** What a question about a link is about: both ends and their ports. */
export function linkContext(a: string, aPort: string | undefined, b: string, bPort: string | undefined): string {
  const end = (n: string, p?: string) => (p && p.trim() ? `${n} ${p.trim()}` : n);
  return `link ${end(a, aPort)} to ${end(b, bPort)}`;
}
