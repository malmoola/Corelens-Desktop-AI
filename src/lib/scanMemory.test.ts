import { describe, expect, it } from 'vitest';

import { loginKindsFromSetting, loginKindsToSetting, oneOf } from './scanMemory';

const OFFERED = ['router', 'switch', 'firewall', 'wireless-controller', 'server'] as const;

describe('what a run may log in to, kept as a setting', () => {
  it('comes back as it was left', () => {
    const kept = loginKindsToSetting(['switch', 'wireless-controller']);
    expect(loginKindsFromSetting(kept, OFFERED)).toEqual(['switch', 'wireless-controller']);
  });

  it('tells none at all from nothing kept', () => {
    expect(loginKindsFromSetting(loginKindsToSetting([]), OFFERED)).toEqual([]);
    expect(loginKindsFromSetting(undefined, OFFERED)).toBeNull();
    expect(loginKindsFromSetting('', OFFERED)).toBeNull();
  });

  it('keeps only kinds the form offers, and nothing from a setting it cannot read', () => {
    expect(loginKindsFromSetting('["switch","toaster"]', OFFERED)).toEqual(['switch']);
    expect(loginKindsFromSetting('not json', OFFERED)).toBeNull();
    expect(loginKindsFromSetting('{"switch":true}', OFFERED)).toBeNull();
  });

  it('takes a kept choice only if it is one of those there are', () => {
    expect(oneOf('classic', ['collector', 'classic'])).toBe('classic');
    expect(oneOf('telnet', ['ssh', 'telnet', 'sshThenTelnet'])).toBe('telnet');
    expect(oneOf('rsh', ['ssh', 'telnet', 'sshThenTelnet'])).toBeNull();
    expect(oneOf(undefined, ['collector', 'classic'])).toBeNull();
  });
});
