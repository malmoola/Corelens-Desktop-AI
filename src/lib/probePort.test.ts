import { describe, expect, it } from 'vitest';

import { defaultPort, portForKind } from './probePort';

describe("a check's port", () => {
  it('is the number the form shows once the type needs one', () => {
    // A new check has none; choosing TCP or UDP gives it the one on screen.
    expect(portForKind('icmp', null, 'tcp')).toBe(443);
    expect(portForKind('icmp', undefined, 'udp')).toBe(53);
    expect(defaultPort('tcp')).toBe(443);
    expect(defaultPort('udp')).toBe(53);
    expect(defaultPort('http')).toBe(80);
    expect(defaultPort('https')).toBe(443);
  });

  it('follows the type while it is still the type\'s own', () => {
    expect(portForKind('tcp', 443, 'udp')).toBe(53);
    expect(portForKind('udp', 53, 'tcp')).toBe(443);
    // HTTP and HTTPS have their engine's own; a ping has none.
    expect(portForKind('tcp', 443, 'http')).toBeNull();
    expect(portForKind('tcp', 443, 'icmp')).toBeNull();
    expect(portForKind('http', null, 'https')).toBeNull();
  });

  it('is kept when somebody typed it', () => {
    expect(portForKind('tcp', 8443, 'udp')).toBe(8443);
    expect(portForKind('tcp', 22, 'http')).toBe(22);
    expect(portForKind('udp', 123, 'tcp')).toBe(123);
  });
});
