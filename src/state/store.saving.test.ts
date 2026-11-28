/**
 * A save takes time, and work goes on while it does.
 *
 * The write to disk, and for a project kept in a company folder the write
 * to the share, are waited for. Whatever is drawn or typed in that time is
 * not in what was written: it must still count as unsaved when the save
 * comes back, and a save that comes back after its project was closed, or
 * another opened, must not touch what is open now.
 */
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

import { ipc } from '../lib/ipc';
import { useStore } from './store';

const meta = { id: 'p-save', name: 'Site A', customer: '', site: '', ticket: '', engineer: '', description: '', createdAt: 1, updatedAt: 1, archived: false };

/** A write that finishes when the test says so. */
function heldWrite() {
  let finish: () => void = () => undefined;
  const written: unknown[] = [];
  vi.spyOn(ipc, 'saveProject').mockImplementation((payload) => {
    written.push(payload);
    return new Promise<void>((resolve) => {
      finish = resolve;
    });
  });
  vi.spyOn(ipc, 'listProjects').mockResolvedValue([]);
  vi.spyOn(ipc, 'listProjectFolders').mockResolvedValue({ folders: [], placement: {} });
  return { finish: () => finish(), written };
}

afterEach(() => {
  vi.restoreAllMocks();
  useStore.setState({ meta: null, dirty: false });
});

describe('unsaved work on offer after a crash', () => {
  const slotKey = 'corelens.recovery.p-save';
  // The tests run without a browser; the slot is kept in the browser's
  // storage, so one is stood in for it here.
  const kept = new Map<string, string>();
  const standIn = { getItem: (k: string) => kept.get(k) ?? null, setItem: (k: string, v: string) => void kept.set(k, v), removeItem: (k: string) => void kept.delete(k) };
  const had = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  beforeAll(() => Object.defineProperty(globalThis, 'localStorage', { value: standIn, configurable: true }));
  afterAll(() => {
    if (had) Object.defineProperty(globalThis, 'localStorage', had);
    else delete (globalThis as { localStorage?: unknown }).localStorage;
  });
  const offer = () => {
    const doc = useStore.getState().doc;
    const recovered = { ...doc, pages: doc.pages.map((p, n) => (n === 0 ? { ...p, name: 'What was being drawn' } : p)) };
    localStorage.setItem(slotKey, JSON.stringify({ savedAt: 50, document: recovered }));
    useStore.setState({ meta, dirty: false, recovery: { savedAt: 50 } });
  };

  it('is still there to be restored after a save made before it was answered', async () => {
    vi.spyOn(ipc, 'saveProject').mockResolvedValue();
    vi.spyOn(ipc, 'listProjects').mockResolvedValue([]);
    vi.spyOn(ipc, 'listProjectFolders').mockResolvedValue({ folders: [], placement: {} });
    offer();
    // Something small done while the offer stands, and autosave follows.
    useStore.getState().addPage('Looked around first');
    await useStore.getState().saveProject({ auto: true });
    expect(localStorage.getItem(slotKey), 'the save threw the offered work away').not.toBeNull();

    useStore.getState().restoreRecovery();
    expect(useStore.getState().doc.pages[0]?.name).toBe('What was being drawn');
    expect(useStore.getState().recovery).toBeNull();
    expect(localStorage.getItem(slotKey), 'answered, the slot is done with').toBeNull();
  });

  it('says so when there is nothing left to restore', () => {
    offer();
    localStorage.removeItem(slotKey);
    useStore.setState({ statusMessage: null });
    useStore.getState().restoreRecovery();
    expect(useStore.getState().recovery).toBeNull();
    expect(useStore.getState().statusMessage ?? '', 'Restore did nothing and said nothing').toMatch(/could not be found|nothing was restored/i);
  });
});

describe('a save that is still being written', () => {
  it('leaves an edit made meanwhile unsaved', async () => {
    const write = heldWrite();
    useStore.setState({ meta, dirty: true });
    const pagesBefore = useStore.getState().doc.pages.length;

    const saving = useStore.getState().saveProject({ auto: true });
    // Drawn while the write is away.
    useStore.getState().addPage('Drawn during the save');
    expect(useStore.getState().doc.pages.length).toBe(pagesBefore + 1);
    write.finish();
    await saving;

    const after = useStore.getState();
    expect((write.written[0] as { document: { pages: unknown[] } }).document.pages.length, 'what was written is what there was when the save began').toBe(pagesBefore);
    expect(after.dirty, 'the page added during the save is not on disk').toBe(true);
    expect(after.doc.pages.length).toBe(pagesBefore + 1);
  });

  it('keeps a project detail typed meanwhile, and still counts it unsaved', async () => {
    const write = heldWrite();
    useStore.setState({ meta, dirty: true });

    const saving = useStore.getState().saveProject({ auto: true });
    useStore.getState().updateMeta({ site: 'Building 2' });
    write.finish();
    await saving;

    const after = useStore.getState();
    expect(after.meta?.site, 'the detail typed during the save was put back').toBe('Building 2');
    expect(after.dirty).toBe(true);
    expect(after.meta?.updatedAt, 'the time of the save is still recorded').toBeGreaterThan(1);
  });

  it('marks the project saved when nothing changed meanwhile', async () => {
    const write = heldWrite();
    useStore.setState({ meta, dirty: true });
    const saving = useStore.getState().saveProject();
    write.finish();
    await saving;
    expect(useStore.getState().dirty).toBe(false);
    expect(useStore.getState().lastSavedAt).toBe(useStore.getState().meta?.updatedAt);
  });

  it('touches nothing when another project is open by the time it comes back', async () => {
    const write = heldWrite();
    useStore.setState({ meta, dirty: true });
    const saving = useStore.getState().saveProject({ auto: true });
    // Another project opened while the first one's write is away.
    const other = { ...meta, id: 'p-other', name: 'Site B', updatedAt: 7 };
    useStore.setState({ meta: other, dirty: true });
    write.finish();
    await saving;

    const after = useStore.getState();
    expect(after.meta?.id, 'the old project was put back over the open one').toBe('p-other');
    expect(after.meta?.updatedAt).toBe(7);
    expect(after.dirty, 'the open project was marked saved by another project\'s save').toBe(true);
  });

  it('touches nothing when the project was closed by the time it comes back', async () => {
    const write = heldWrite();
    useStore.setState({ meta, dirty: true });
    const saving = useStore.getState().saveProject({ auto: true });
    useStore.setState({ meta: null, dirty: false });
    write.finish();
    await saving;
    expect(useStore.getState().meta, 'a closed project came back').toBeNull();
  });
});
