import { describe, expect, it } from 'vitest';

import { applyEvent, changeIdOf, describeCall, deviceContext, isChatShortcut, itemsFromHistory, linkContext, parseTable, tokens, type ChatItem } from './aiChat';
import type { AiChatRow } from './ipc';

const row = (message: AiChatRow['message'], id = 1): AiChatRow => ({ id, atMs: 0, provider: 'anthropic', model: 'claude-opus-5-5', usage: { inputTokens: 0, outputTokens: 0 }, message });

describe('the chat panel', () => {
  it('pairs each tool call with its result and keeps the order', () => {
    const items = itemsFromHistory([
      row({ role: 'user', text: 'is SW1 ok?' }),
      row({ role: 'assistant', text: 'Checking.', provider: 'anthropic', calls: [{ id: 't1', name: 'run_show_command', arguments: { target: 'SW1', command: 'show ip arp' } }] }),
      row({ role: 'tools', results: [{ id: 't1', name: 'run_show_command', content: 'SW1 (192.0.2.10) · Cisco IOS\nParsed with cisco_ios_show_ip_arp.textfsm (1 row(s)):\n{"ADDRESS":"192.0.2.1","INTERFACE":"Vlan10"}\nOutput (2 line(s)):\n...', isError: false }] }),
      row({ role: 'assistant', text: 'Yes.', provider: 'anthropic', calls: [] }),
    ]);
    expect(items.map((i) => i.kind)).toEqual(['user', 'assistant', 'tool', 'assistant']);
    const tool = items[2] as Extract<ChatItem, { kind: 'tool' }>;
    expect([tool.target, tool.command]).toEqual(['SW1', 'show ip arp']);
    expect(tool.table).toEqual({ columns: ['ADDRESS', 'INTERFACE'], rows: [['192.0.2.1', 'Vlan10']] });
  });

  it('builds a running turn from its events, text after a call starting a new answer', () => {
    let items: ChatItem[] = [{ kind: 'user', text: 'q' }];
    const ev = (e: object) => ({ projectId: 'p1', ...e }) as Parameters<typeof applyEvent>[1];
    items = applyEvent(items, ev({ kind: 'text', text: 'Let me ' }));
    items = applyEvent(items, ev({ kind: 'text', text: 'look.' }));
    items = applyEvent(items, ev({ kind: 'toolCall', id: 'c1', name: 'propose_config_change', target: 'SW1', command: 'interface Gi1/0/24 /  description X' }));
    items = applyEvent(items, ev({ kind: 'toolResult', id: 'c1', name: 'propose_config_change', isError: false, text: 'Change 1a2b3c4d (configuration change): approved and done.' }));
    items = applyEvent(items, ev({ kind: 'text', text: 'Done.' }));
    expect(items.map((i) => (i.kind === 'assistant' ? i.text : i.kind))).toEqual(['user', 'Let me look.', 'tool', 'Done.']);
    expect((items[2] as Extract<ChatItem, { kind: 'tool' }>).changeId).toBe('1a2b3c4d');
    expect(applyEvent(items, ev({ kind: 'error', message: 'Rate limited' })).at(-1)).toEqual({ kind: 'notice', text: 'Rate limited', tone: 'error' });
  });

  it('names a batch by its devices and a proposal by its lines', () => {
    expect(describeCall({ batch: [{ target: 'A' }, { target: 'B' }], commands: [] })).toEqual({ target: 'A, B', command: '' });
    expect(describeCall({ target: 'SW1', commands: ['interface Gi1/0/1', ' shutdown'] })).toEqual({ target: 'SW1', command: 'interface Gi1/0/1 /  shutdown' });
    expect(describeCall({ query: 'Vlan10' })).toEqual({ target: '', command: 'Vlan10' });
  });

  it('reads no table where there is none, and no change where none is named', () => {
    expect(parseTable('Output (3 line(s)):\nfoo')).toBeNull();
    expect(changeIdOf('the vault in Corelens is locked')).toBeNull();
  });

  it('counts tokens briefly and opens on Ctrl+Shift+A only', () => {
    expect([tokens(950), tokens(12_345), tokens(250_000)]).toEqual(['950', '12.3k', '250k']);
    expect(isChatShortcut({ key: 'A', ctrlKey: true, metaKey: false, shiftKey: true, altKey: false })).toBe(true);
    expect(isChatShortcut({ key: 'a', ctrlKey: true, metaKey: false, shiftKey: false, altKey: false })).toBe(false);
  });

  it('names a device by its primary address and a link by both ends', () => {
    expect(deviceContext({ label: 'SW1', addresses: [{ address: '192.0.2.9' }, { address: '192.0.2.10', isPrimary: true }] })).toBe('device SW1 (192.0.2.10)');
    expect(deviceContext({ hostname: 'rtr1.example.test' })).toBe('device rtr1.example.test');
    expect(linkContext('SW1', 'Gi1/0/1', 'RTR1', '')).toBe('link SW1 Gi1/0/1 to RTR1');
  });
});
