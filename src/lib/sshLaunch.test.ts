import { describe, expect, it } from 'vitest';

import { MAX_JUMPS, jumpCandidates, planSsh, sshAddress, sshLabel } from './sshLaunch';
import type { DeviceNodeData } from '../types/domain';

const device = (over: Partial<DeviceNodeData> = {}): DeviceNodeData =>
  ({ label: 'SW-A', deviceType: 'access-switch', tags: [], ...over }) as DeviceNodeData;

const at = (address: string, isPrimary = false) => ({ id: address, label: '', address, isPrimary });

describe('which address a session dials', () => {
  it('takes the primary one, the same one the device’s own check watches', () => {
    expect(sshAddress(device({ addresses: [at('192.0.2.9'), at('192.0.2.10', true)] }))).toBe('192.0.2.10');
  });

  it('falls back to the first that has anything in it', () => {
    expect(sshAddress(device({ addresses: [at('  '), at('192.0.2.11')] }))).toBe('192.0.2.11');
  });

  it('has nothing to dial when the device has no address', () => {
    expect(sshAddress(device({ addresses: [] }))).toBe('');
  });
});

describe('what the tab is called', () => {
  it('prefers the name the device gave itself', () => {
    expect(sshLabel(device({ hostname: 'CORE-SW1', addresses: [at('192.0.2.10')] }))).toBe('CORE-SW1');
  });

  it('then the label on the diagram, then the address', () => {
    expect(sshLabel(device({ addresses: [at('192.0.2.10')] }))).toBe('SW-A');
    expect(sshLabel(device({ label: '', addresses: [at('192.0.2.10')] }))).toBe('192.0.2.10');
  });
});

describe('what stops a session before anything is sent', () => {
  it('says which half is missing rather than letting it time out', () => {
    expect(planSsh(device({ addresses: [] }))).toEqual({ ok: false, reason: 'noAddress' });
    expect(planSsh(device({ addresses: [at('192.0.2.10')] }))).toEqual({ ok: false, reason: 'noCredential' });
  });

  it('and hands over the address, the credential id and the name when both are there', () => {
    expect(planSsh(device({ hostname: 'CORE-SW1', addresses: [at('192.0.2.10', true)], sshCredentialId: 'cred-1' })))
      .toEqual({ ok: true, address: '192.0.2.10', credentialId: 'cred-1', label: 'CORE-SW1', inherited: false });
  });

  it('does not accept a credential id that is only spaces', () => {
    expect(planSsh(device({ addresses: [at('192.0.2.10')], sshCredentialId: '   ' })))
      .toEqual({ ok: false, reason: 'noCredential' });
  });
});

describe('a device with no login of its own inherits the project\u2019s', () => {
  const plain = device({ addresses: [at('192.0.2.10', true)] });

  it('falls back to the project, and says that is what happened', () => {
    expect(planSsh(plain, 'project-cred')).toEqual({
      ok: true, address: '192.0.2.10', credentialId: 'project-cred', label: 'SW-A', inherited: true,
    });
  });

  it('but the device\u2019s own wins, which is what makes it an override', () => {
    const own = device({ addresses: [at('192.0.2.10', true)], sshCredentialId: 'its-own' });
    expect(planSsh(own, 'project-cred')).toMatchObject({ credentialId: 'its-own', inherited: false });
  });

  it('and with neither there is still nothing to try', () => {
    expect(planSsh(plain, '   ')).toEqual({ ok: false, reason: 'noCredential' });
    expect(planSsh(plain)).toEqual({ ok: false, reason: 'noCredential' });
  });
});

describe('which devices a session can go through', () => {
  const dev = (id: string, label: string, address = ''): { id: string; type: string; data: DeviceNodeData } =>
    ({ id, type: 'device', data: { label, addresses: address ? [{ id: `a-${id}`, label: 'Management', address, isPrimary: true }] : [] } as unknown as DeviceNodeData });
  const nodes = [dev('leaf', 'LEAF-04', '10.66.250.14'), dev('s2', 'SPINE-2', '192.0.2.2'), dev('s1', 'SPINE-1', '192.0.2.1'), dev('ap', 'AP-1'), dev('far', 'FAR', '192.0.2.9'), dev('lone', 'LONE', '192.0.2.50'), { id: 'n', type: 'note', data: {} as DeviceNodeData }];
  const edges = [
    { source: 'leaf', target: 's2' },
    { source: 's1', target: 'leaf' },
    { source: 's1', target: 'leaf' },
    { source: 'leaf', target: 'ap' },
    { source: 'leaf', target: 'n' },
    { source: 's1', target: 'far' },
  ];

  it('offers the devices linked to it that have an address, once each, by name', () => {
    expect(jumpCandidates('leaf', nodes, edges)).toEqual([
      { nodeId: 's1', label: 'SPINE-1', address: '192.0.2.1' },
      { nodeId: 's2', label: 'SPINE-2', address: '192.0.2.2' },
    ]);
  });

  it('offers nothing for a device with no linked neighbour it could go through', () => {
    expect(jumpCandidates('lone', nodes, edges)).toEqual([]);
    // An address-less neighbour is never offered, but a device without one
    // can still be reached through a neighbour that has one.
    expect(jumpCandidates('ap', nodes, edges).map((j) => j.label)).toEqual(['LEAF-04']);
  });

  it('never more than a menu can hold', () => {
    const many = Array.from({ length: 9 }, (_, i) => dev(`d${i}`, `D-${i}`, `192.0.2.${10 + i}`));
    const spokes = many.map((d) => ({ source: 'hub', target: d.id }));
    expect(jumpCandidates('hub', [dev('hub', 'HUB', '192.0.2.100'), ...many], spokes)).toHaveLength(MAX_JUMPS);
  });
});
