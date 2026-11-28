import { describe, expect, it } from 'vitest';

import { maintenanceKey, probesForEngine } from './enginePlan';

const node = (id: string, maintenance = false) => ({ id, type: 'device', position: { x: 0, y: 0 }, data: { label: id, maintenance } });
const edge = (id: string, maintenance = false) => ({ id, source: 'a', target: 'b', data: { maintenance } });
const probe = (id: string, objectId: string, over: Record<string, unknown> = {}) =>
  ({ id, projectId: '', objectKind: 'node', objectId, name: id, kind: 'icmp', target: '192.0.2.1', intervalSeconds: 5, timeoutMs: 1000, failureThreshold: 3, recoveryThreshold: 1, enabled: true, ...over });
const doc = (nodes: unknown[], edges: unknown[], probes: unknown[]) =>
  ({ pages: [{ id: 'p1', name: 'One', nodes, edges, canvas: {} }], probes }) as never;

describe('the checks as the engine is given them', () => {
  it('carry the maintenance of the device or link they check', () => {
    const d = doc(
      [node('a', true), node('b')],
      [edge('e1', true), edge('e2')],
      [probe('pa', 'a'), probe('pb', 'b'), probe('pe1', 'e1', { objectKind: 'link' }), probe('pe2', 'e2', { objectKind: 'link' }), probe('own', 'b', { maintenance: true })],
    );
    const sent = probesForEngine(d, 'proj');
    expect(Object.fromEntries(sent.map((p) => [p.id, p.maintenance]))).toEqual({ pa: true, pb: false, pe1: true, pe2: false, own: true });
    expect(sent.every((p) => p.projectId === 'proj')).toBe(true);
  });

  it('leave out what the engine does not run', () => {
    const d = doc([node('a')], [], [probe('off', 'a', { enabled: false }), probe('by-hand', 'a', { kind: 'manual' }), probe('on', 'a')]);
    expect(probesForEngine(d, 'proj').map((p) => p.id)).toEqual(['on']);
  });

  it('are sent again when a checked thing goes into or out of maintenance, and not for one with no check', () => {
    const before = maintenanceKey(doc([node('a'), node('b'), node('unchecked')], [], [probe('pa', 'a'), probe('pb', 'b')]));
    const after = maintenanceKey(doc([node('a', true), node('b'), node('unchecked')], [], [probe('pa', 'a'), probe('pb', 'b')]));
    const other = maintenanceKey(doc([node('a'), node('b'), node('unchecked', true)], [], [probe('pa', 'a'), probe('pb', 'b')]));
    expect(before).toBe('');
    expect(after).not.toBe(before);
    expect(other, 'a device with no check changes nothing the engine runs').toBe(before);
  });
});
