import { describe, expect, it } from 'vitest';

import { answered, missesAfter, wasCarriedOut } from './probeOutcome';

describe('the count of misses beside a check', () => {
  it('goes up for a reply that did not come', () => {
    expect(missesAfter('timeout', 0)).toBe(1);
    expect(missesAfter('unreachable', 1)).toBe(2);
    expect(missesAfter('no_answer', 2)).toBe(3);
  });

  it('is nothing for a device that answered, restarted or not', () => {
    expect(missesAfter('success', 2)).toBe(0);
    // It answered: its uptime had gone back, which is a warning, not a miss.
    expect(missesAfter('restarted', 2)).toBe(0);
    expect(answered('restarted')).toBe(true);
  });

  it('does not count a check that could not be made', () => {
    expect(missesAfter('invalid_target', 2)).toBe(0);
    expect(missesAfter('not_run', 2)).toBe(0);
    expect(wasCarriedOut('not_run')).toBe(false);
    expect(wasCarriedOut('os_error')).toBe(true);
  });
});
