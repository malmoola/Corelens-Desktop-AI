import { describe, expect, it } from 'vitest';

import { coverage, HISTORY_LIMIT } from './historyWindow';

describe("how much of a window a check's history covers", () => {
  const at = (n: number, from = 1_000) => Array.from({ length: n }, (_, i) => ({ timestampMs: from + i * 5_000 }));

  it('is the whole of it when fewer results came back than were asked for', () => {
    expect(coverage(at(720), HISTORY_LIMIT)).toEqual({ cut: false, fromMs: 1_000 });
  });

  it('is cut, and says from when, when as many came back as were asked for', () => {
    const c = coverage(at(HISTORY_LIMIT, 50_000), HISTORY_LIMIT);
    expect(c.cut).toBe(true);
    expect(c.fromMs).toBe(50_000);
  });

  it('finds the oldest whatever order they came in', () => {
    expect(coverage([{ timestampMs: 9 }, { timestampMs: 3 }, { timestampMs: 7 }], 10).fromMs).toBe(3);
  });

  it('claims nothing with no results', () => {
    expect(coverage([], HISTORY_LIMIT)).toEqual({ cut: false, fromMs: null });
  });

  it('asks for enough to cover a day at the default interval', () => {
    expect(HISTORY_LIMIT).toBeGreaterThanOrEqual(86_400 / 5);
  });
});
