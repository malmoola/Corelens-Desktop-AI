/**
 * What is selected is what Delete removes, and nothing else.
 *
 * The canvas marks the selected objects themselves; the store also keeps
 * the one object the inspector shows. The two drifted apart: a device
 * clicked, then others chosen with a box, was still "the selected one" and
 * went with them at the next Delete; a locked device kept in place lost
 * its checks; a link removed along with its device left its checks behind,
 * still running with nothing on the page to show or remove them.
 */
import { afterEach, describe, expect, it } from 'vitest';

import { useStore } from './store';

const dev = (id: string, over: Record<string, unknown> = {}, data: Record<string, unknown> = {}) =>
  ({ id, type: 'device', position: { x: 0, y: 0 }, data: { label: id.toUpperCase(), deviceType: 'access-switch', tags: [], addresses: [], ...data }, ...over }) as never;
const link = (id: string, source: string, target: string, over: Record<string, unknown> = {}) =>
  ({ id, source, target, type: 'live', data: {}, ...over }) as never;
const probe = (id: string, objectId: string, objectKind: 'node' | 'link') =>
  ({ id, objectId, objectKind, name: id, kind: 'icmp', target: '192.0.2.1', enabled: true }) as never;

function page(nodes: unknown[], edges: unknown[] = [], probes: unknown[] = []) {
  useStore.setState((st) => ({
    doc: { ...st.doc, probes: probes as never, pages: st.doc.pages.map((p, n) => (n === 0 ? { ...p, nodes: nodes as never, edges: edges as never } : p)) },
    selectedNodeId: null,
    selectedEdgeId: null,
    dirty: false,
    past: [],
    future: [],
  }));
}
const nodes = () => useStore.getState().doc.pages[0]!.nodes.map((n) => n.id).sort().join(',');
const edges = () => useStore.getState().doc.pages[0]!.edges.map((e) => e.id).sort().join(',');
const probes = () => useStore.getState().doc.probes.map((p) => p.id).sort().join(',');

afterEach(() => page([]));

describe('Delete', () => {
  it('leaves a device that was selected before and is not now', () => {
    page([dev('x'), dev('a', { selected: true }), dev('b', { selected: true })]);
    // Clicked earlier, then two others chosen with a box.
    useStore.setState({ selectedNodeId: 'x' });
    useStore.getState().deleteSelected();
    expect(nodes()).toBe('x');
  });

  it('leaves a link that was selected before and is not now', () => {
    page([dev('a'), dev('b'), dev('c', { selected: true })], [link('e1', 'a', 'b')]);
    useStore.setState({ selectedEdgeId: 'e1' });
    useStore.getState().deleteSelected();
    expect(nodes()).toBe('a,b');
    expect(edges()).toBe('e1');
  });

  it('still removes the one object chosen by name when nothing is marked', () => {
    // A row of a table chooses a device without marking it on the canvas.
    page([dev('x'), dev('a')], [link('e1', 'x', 'a')]);
    useStore.getState().select('x', null);
    useStore.getState().deleteSelected();
    expect(nodes()).toBe('a');
    page([dev('x'), dev('a')], [link('e1', 'x', 'a')]);
    useStore.getState().select(null, 'e1');
    useStore.getState().deleteSelected();
    expect(nodes()).toBe('a,x');
    expect(edges()).toBe('');
  });

  it('keeps the checks of a locked device it leaves in place', () => {
    page(
      [dev('locked', { selected: true }, { locked: true }), dev('n', { selected: true }), dev('m')],
      [link('e1', 'n', 'm'), link('e2', 'locked', 'm')],
      [probe('p-locked', 'locked', 'node'), probe('p-n', 'n', 'node'), probe('p-e1', 'e1', 'link'), probe('p-e2', 'e2', 'link'), probe('p-m', 'm', 'node')],
    );
    useStore.getState().deleteSelected();
    expect(nodes()).toBe('locked,m');
    expect(edges(), 'the link of the removed device goes with it').toBe('e2');
    expect(probes(), 'the locked device keeps its check; the removed link takes its own with it').toBe('p-e2,p-locked,p-m');
  });

  it('does nothing at all, and takes no undo step, with nothing selected', () => {
    page([dev('a'), dev('b')]);
    useStore.setState({ future: [{ label: 'kept' } as never] });
    useStore.getState().deleteSelected();
    const after = useStore.getState();
    expect(nodes()).toBe('a,b');
    expect(after.past.length, 'an empty step was put on the undo list').toBe(0);
    expect(after.future.length, 'what could be redone was thrown away').toBe(1);
    expect(after.dirty, 'the project was marked unsaved').toBe(false);
  });
});

describe('the one object the inspector shows', () => {
  it('follows the marks the canvas puts on objects', () => {
    page([dev('x', { selected: true }), dev('a'), dev('b')]);
    useStore.setState({ selectedNodeId: 'x' });
    const change = (id: string, selected: boolean) => ({ type: 'select' as const, id, selected });
    // A box drawn round two others.
    useStore.getState().onNodesChange([change('x', false), change('a', true), change('b', true)]);
    expect(useStore.getState().selectedNodeId, 'a device no longer marked is still the one shown').toBeNull();
    // A box round one.
    useStore.getState().onNodesChange([change('b', false)]);
    expect(useStore.getState().selectedNodeId).toBe('a');
  });

  it('is the only thing marked after selectOnly', () => {
    page([dev('a', { selected: true }), dev('b', { selected: true }), dev('c', { selected: true }), dev('d')], [link('e1', 'a', 'b', { selected: true })]);
    useStore.getState().selectOnly('d', null);
    const pg = useStore.getState().doc.pages[0]!;
    expect(pg.nodes.filter((n) => n.selected).map((n) => n.id)).toEqual(['d']);
    expect(pg.edges.filter((e) => e.selected)).toEqual([]);
    expect(useStore.getState().selectedNodeId).toBe('d');
    useStore.getState().deleteSelected();
    expect(nodes(), 'a right-click Delete on one object took the rest of the selection with it').toBe('a,b,c');
  });
});

describe('choosing a link', () => {
  it('does not make the project unsaved', () => {
    page([dev('a'), dev('b')], [link('e1', 'a', 'b')]);
    useStore.getState().onEdgesChange([{ type: 'select', id: 'e1', selected: true }]);
    expect(useStore.getState().doc.pages[0]!.edges[0]!.selected).toBe(true);
    expect(useStore.getState().dirty).toBe(false);
  });
});

describe('a paste', () => {
  it('leaves nothing of the original selected, so Delete takes the paste alone', () => {
    page([dev('a', { selected: true }), dev('b', { selected: true })], [link('e1', 'a', 'b', { selected: true })]);
    useStore.getState().copySelection();
    expect(useStore.getState().paste()).toBe(2);
    const pg = useStore.getState().doc.pages[0]!;
    expect(pg.edges.find((e) => e.id === 'e1')?.selected ?? false, 'the original link is still selected').toBe(false);
    useStore.getState().deleteSelected();
    expect(nodes()).toBe('a,b');
    expect(edges(), 'discarding the paste deleted the original link').toBe('e1');
  });
});
