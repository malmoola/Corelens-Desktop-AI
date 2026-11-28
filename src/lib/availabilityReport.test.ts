import { describe, expect, it } from 'vitest';

import { availabilityHtml, availabilityModel, monitoredSpans } from './availabilityReport';
import type { EventRow, HealthStatus } from '../types/domain';

const T0 = Date.UTC(2026, 0, 5, 12, 0, 0);
const min = 60_000;
let n = 0;
const ev = (objectId: string, objectName: string, at: number, from: HealthStatus | null, to: HealthStatus, message = '', target = '192.0.2.10'): EventRow => ({
  id: `e${++n}`, projectId: 'p', sessionId: 's', timestampMs: T0 + at, objectType: 'node', objectId, objectName,
  eventType: 'transition', previousStatus: from, currentStatus: to, probeType: 'tcp', target, rttMs: null, message,
});

const events: EventRow[] = [
  // Three services in one group fail together at +60 min and come back.
  ...['Portal', 'Mail', 'Files'].flatMap((s, i) => [
    ev(s, `PROD · ${s} · app${i}:443`, 0, 'unknown', 'healthy', 'Connected'),
    ev(s, `PROD · ${s} · app${i}:443`, 60 * min + i * 1000, 'healthy', 'down', 'timed out'),
    ev(s, `PROD · ${s} · app${i}:443`, 62 * min + i * 1000, 'down', 'healthy', 'Connected'),
  ]),
  // A device that is slow once.
  ev('sw1', 'CORE-SW1', 0, 'unknown', 'healthy', 'Reply, 2 ms', '192.0.2.1'),
  ev('sw1', 'CORE-SW1', 30 * min, 'healthy', 'warning', 'Reply, 300 ms', '192.0.2.1'),
  ev('sw1', 'CORE-SW1', 31 * min, 'warning', 'healthy', 'Reply, 2 ms', '192.0.2.1'),
  // Down the whole time.
  ev('old', 'OLD-SW', 0, 'unknown', 'down', 'Request timed out', '192.0.2.99'),
  // A target that never resolved.
  ev('mac', 'Some NIC', 0, 'unknown', 'down', 'Host name could not be resolved', '0000.5e00.5301'),
  // Not a transition: ignored.
  { ...ev('sw1', 'CORE-SW1', 5 * min, null, 'healthy'), eventType: 'session' as const },
];
const END = T0 + 120 * min;

describe('availabilityModel', () => {
  const m = availabilityModel(events, END);

  it('groups service checks by the first part of their name, devices last', () => {
    expect(m.groups.map((g) => g.name)).toEqual(['PROD', 'Devices']);
    expect(m.groups[0]!.checks.map((c) => c.label)).toEqual(['Files · app2:443', 'Mail · app1:443', 'Portal · app0:443']);
  });

  it('works out each check’s periods, downtime and availability over its own observed time', () => {
    const portal = m.counted.find((c) => c.id === 'Portal')!;
    expect(portal.periods.map((p) => p.status)).toEqual(['healthy', 'down', 'healthy']);
    expect(portal.downMs).toBe(2 * min);
    expect(portal.availability).toBeCloseTo(100 * (1 - 2 / 120), 5);
    expect(portal.now).toBe('healthy');
    const sw = m.counted.find((c) => c.id === 'sw1')!;
    expect(sw.slowMs).toBe(min);
    expect(sw.availability).toBe(100);
  });

  it('finds the outages that began together as one wide event, back when the last one was', () => {
    expect(m.wide).toHaveLength(1);
    expect(m.wide[0]!.checks).toHaveLength(3);
    expect(m.wide[0]!.fromMs).toBe(T0 + 60 * min);
    expect(m.wide[0]!.toMs).toBe(T0 + 62 * min + 2000);
  });

  it('lists what was down throughout, and sets apart what never ran', () => {
    expect(m.alwaysDown.map((c) => c.name)).toEqual(['OLD-SW']);
    expect(m.neverRan.map((c) => c.name)).toEqual(['Some NIC']);
    expect(m.counted.some((c) => c.name === 'Some NIC')).toBe(false);
    expect(m.fromMs).toBe(T0);
  });

  it('three unrelated outages hours apart are not a wide event', () => {
    const apart = ['A', 'B', 'C'].flatMap((s, i) => [
      ev(s, s, 0, 'unknown', 'healthy'),
      ev(s, s, (10 + i * 30) * min, 'healthy', 'down'),
      ev(s, s, (11 + i * 30) * min, 'down', 'healthy'),
    ]);
    expect(availabilityModel(apart, END).wide).toEqual([]);
  });
});

describe('availabilityHtml', () => {
  const html = availabilityHtml(availabilityModel(events, END), {
    title: 'Availability <test>',
    subtitle: 'Example',
    zone: 'Zulu (UTC)',
    when: (ms) => new Date(ms).toISOString(),
    clock: (ms) => new Date(ms).toISOString().slice(11, 19),
  });

  it('is one self-contained page that fetches nothing', () => {
    expect(html.startsWith('<!doctype html>')).toBe(true);
    expect(html).not.toMatch(/(src|href)="https?:/);
    expect(html).toContain('Availability &lt;test&gt;');
  });

  it('carries every part, each check with its timeline and durations', () => {
    for (const part of ['tiles', 'wide', 'always-down', 'availability', 'never-ran']) expect(html).toContain(`data-part="${part}"`);
    expect(html).toContain('data-check="PROD · Portal · app0:443"');
    expect(html).toMatch(/12:00:00[\s\S]*?Down[\s\S]*?2m/);
    expect(html).toContain('98.33%');
    expect(html).toContain('Times in Zulu (UTC)');
  });
});

describe('time when checks were not running', () => {
  const hour = 60 * min;
  // Monday 09:00 to 17:00, then Tuesday from 09:00, the report made at 10:00.
  const monday = { id: 'mon', fromMs: T0, toMs: T0 + 8 * hour };
  const tuesday = { id: 'tue', fromMs: T0 + 24 * hour, toMs: T0 + 25 * hour };
  const at = (session: string, e: EventRow): EventRow => ({ ...e, sessionId: session });
  const twoDays: EventRow[] = [
    at('mon', ev('fw', 'FW-01', 3_000, 'unknown', 'healthy', 'Reply, 2 ms', '192.0.2.1')),
    // Down ten minutes before the checks were stopped for the night.
    at('mon', ev('fw', 'FW-01', 8 * hour - 10 * min, 'healthy', 'down', 'Request timed out', '192.0.2.1')),
    at('tue', ev('fw', 'FW-01', 24 * hour + 3_000, 'unknown', 'healthy', 'Reply, 2 ms', '192.0.2.1')),
    // One that was up at the stop.
    at('mon', ev('sw', 'SW-01', 3_000, 'unknown', 'healthy', 'Reply, 1 ms', '192.0.2.2')),
    at('tue', ev('sw', 'SW-01', 24 * hour + 3_000, 'unknown', 'healthy', 'Reply, 1 ms', '192.0.2.2')),
  ];

  it('is not counted as the state a check was last seen in', () => {
    const m = availabilityModel(twoDays, T0 + 25 * hour, [monday, tuesday]);
    const fw = m.checks.find((c) => c.name === 'FW-01')!;
    expect(fw.outages.map((o) => o.toMs - o.fromMs), 'the night was counted as an outage').toEqual([10 * min]);
    expect(fw.downMs).toBe(10 * min);
    // Ten minutes down in nine hours watched, less the first seconds of each day.
    const watched = 8 * hour - 3_000 + (hour - 3_000);
    expect(fw.availability).toBeCloseTo(100 * (1 - (10 * min) / watched), 6);
    const gap = fw.periods.find((p) => p.gap);
    expect(gap && [gap.status, gap.fromMs - T0, gap.toMs - T0]).toEqual(['unknown', 8 * hour, 24 * hour + 3_000]);
    expect(fw.now).toBe('healthy');
    // And the one that was up is not said to have been up all night either.
    const sw = m.checks.find((c) => c.name === 'SW-01')!;
    expect(sw.periods.filter((p) => p.gap).length).toBe(1);
    expect(sw.availability).toBe(100);
  });

  it('ends the period when the checks were last running, not when the report is made', () => {
    // Made on Wednesday with nothing running since Tuesday 10:00.
    const m = availabilityModel(twoDays, T0 + 50 * hour, [monday, tuesday]);
    expect(m.toMs - T0, 'the last state was stretched to the moment of the report').toBe(25 * hour);
    const fw = m.checks.find((c) => c.name === 'FW-01')!;
    expect(fw.periods[fw.periods.length - 1]!.toMs - T0).toBe(25 * hour);
    expect(fw.downMs).toBe(10 * min);
  });

  it('says so in the report', () => {
    const html = availabilityHtml(availabilityModel(twoDays, T0 + 25 * hour, [monday, tuesday]), { title: 'T', subtitle: 'S', zone: 'UTC', when: (ms) => new Date(ms).toISOString(), clock: (ms) => new Date(ms).toISOString().slice(11, 16) });
    expect(html).toContain('Checks were not running');
    expect(html).toMatch(/not running is left out/);
  });

  it('is as before for history with no sessions known', () => {
    const m = availabilityModel(twoDays, T0 + 25 * hour);
    expect(m.checks.find((c) => c.name === 'FW-01')!.periods.some((p) => p.gap)).toBe(false);
  });
});

describe('a device with more than one check', () => {
  // A ping and a web check on one switch: the web interface is turned off
  // at +10 and stays off; the ping is slow for a minute at +15.
  const both: EventRow[] = [
    { ...ev('core', 'CORE-SW1', 0, 'unknown', 'healthy', 'Reply, 2 ms', '192.0.2.1'), probeType: 'icmp' },
    { ...ev('core', 'CORE-SW1', 1_000, 'unknown', 'healthy', '200 OK', 'https://192.0.2.1/'), probeType: 'https' },
    { ...ev('core', 'CORE-SW1', 10 * min, 'healthy', 'down', 'Connection refused', 'https://192.0.2.1/'), probeType: 'https' },
    { ...ev('core', 'CORE-SW1', 15 * min, 'healthy', 'warning', 'Reply, 300 ms', '192.0.2.1'), probeType: 'icmp' },
    { ...ev('core', 'CORE-SW1', 16 * min, 'warning', 'healthy', 'Reply, 2 ms', '192.0.2.1'), probeType: 'icmp' },
  ];
  const m = availabilityModel(both, T0 + 60 * min);

  it('has a history for each, not one made of both', () => {
    const mine = m.checks.filter((c) => c.name.startsWith('CORE-SW1'));
    expect(mine.length, 'two checks were folded into one history').toBe(2);
    const web = mine.find((c) => c.probe === 'HTTPS')!;
    const ping = mine.find((c) => c.probe === 'ICMP')!;
    expect(web.now, 'the web check is still down').toBe('down');
    expect(web.downMs).toBe(50 * min);
    expect(ping.now).toBe('healthy');
    expect(ping.downMs).toBe(0);
    expect(ping.slowMs).toBe(1 * min);
    // Each is told apart by name where the device has several.
    expect(new Set(mine.map((c) => c.name)).size).toBe(2);
  });
});

describe('the stretches checks were running', () => {
  const hour = 60 * min;
  const change = (session: string, at: number): EventRow => ({ ...ev('fw', 'FW-01', at, 'healthy', 'down'), sessionId: session });

  it('end at the stop, at now for the one running, and at the last thing recorded for one never stopped', () => {
    const spans = monitoredSpans(
      [
        { id: 'stopped', startedAt: T0, stoppedAt: T0 + 8 * hour },
        { id: 'window-closed', startedAt: T0 + 24 * hour, stoppedAt: null, lastSampleAt: T0 + 30 * hour },
        { id: 'no-results-kept', startedAt: T0 + 48 * hour, stoppedAt: null, lastSampleAt: null },
        { id: 'nothing-at-all', startedAt: T0 + 60 * hour, stoppedAt: null },
        { id: 'running', startedAt: T0 + 72 * hour, stoppedAt: null, lastSampleAt: T0 + 72 * hour + 5_000 },
      ],
      [change('no-results-kept', 49 * hour), change('no-results-kept', 50 * hour), change('stopped', 2 * hour)],
      'running',
      T0 + 73 * hour,
    );
    expect(spans.map((sp) => [sp.id, (sp.toMs - T0) / hour])).toEqual([
      ['stopped', 8],
      ['window-closed', 30],
      ['no-results-kept', 50],
      ['nothing-at-all', 60],
      ['running', 73],
    ]);
  });
});
