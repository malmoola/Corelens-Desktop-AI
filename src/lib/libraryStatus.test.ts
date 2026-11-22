import { describe, expect, it } from 'vitest';

import { readLibraryStatus } from './ipc';

describe('readLibraryStatus', () => {
  it('takes what Rust sends', () => {
    const s = readLibraryStatus({ folders: { project: '/docs', library: null }, projectOpen: true, documents: [{ name: 'R.docx', kind: 'docx', project: true, error: null, runbook: 'R', parts: [{ label: 'Part A', title: 'Part A', steps: 2, done: 0, failed: 0, next: 'A1' }], copyOf: null }] });
    expect(s?.folders.project).toBe('/docs');
    expect(s?.documents[0]?.parts[0]?.next).toBe('A1');
  });

  it('is null for anything else, so a page shows nothing found rather than failing', () => {
    for (const v of [[], null, undefined, 'x', {}, { folders: null, documents: [] }, { folders: {}, documents: 'x' }]) {
      expect(readLibraryStatus(v)).toBeNull();
    }
  });

  it('drops a document that is not one, and fills what is missing', () => {
    const s = readLibraryStatus({ folders: { project: '', library: '/lib' }, documents: [null, 3, { name: 'a.md', kind: 'md' }] });
    expect(s?.folders.project).toBeNull();
    expect(s?.documents).toHaveLength(1);
    expect(s?.documents[0]?.parts).toEqual([]);
    expect(s?.documents[0]?.runbook).toBeNull();
  });
});
