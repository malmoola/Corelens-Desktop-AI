/**
 * A terminal tab can be moved along the strip. Only the order changes:
 * which tab is showing, and every session's id, stay as they were.
 */
import { beforeEach, describe, expect, it } from 'vitest';

import { useStore } from './store';

const tab = (id: string) => ({ id, address: `${id}.example`, label: id, status: 'open' as const });

beforeEach(() => {
  useStore.setState({ sshSessions: [], sshActive: null });
  for (const id of ['a', 'b', 'c', 'd']) useStore.getState().openSshTab(tab(id));
  useStore.getState().setSshActive('b');
});

const order = () => useStore.getState().sshSessions.map((t) => t.id);

describe('moving a terminal tab', () => {
  it('puts a dragged tab where it was dropped and shifts the rest', () => {
    useStore.getState().moveSshTab('c', 0);
    expect(order()).toEqual(['c', 'a', 'b', 'd']);
    useStore.getState().moveSshTab('c', 3);
    expect(order()).toEqual(['a', 'b', 'd', 'c']);
    useStore.getState().moveSshTab('a', 2);
    expect(order()).toEqual(['b', 'd', 'a', 'c']);
  });

  it('keeps the tab that is showing showing', () => {
    useStore.getState().moveSshTab('b', 3);
    expect(useStore.getState().sshActive).toBe('b');
  });

  it('clamps past either end and ignores a tab that has gone', () => {
    useStore.getState().moveSshTab('d', -5);
    expect(order()).toEqual(['d', 'a', 'b', 'c']);
    useStore.getState().moveSshTab('d', 99);
    expect(order()).toEqual(['a', 'b', 'c', 'd']);
    const before = useStore.getState().sshSessions;
    useStore.getState().moveSshTab('gone', 0);
    expect(useStore.getState().sshSessions).toBe(before);
  });
});
