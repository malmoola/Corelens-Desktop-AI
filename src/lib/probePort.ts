/**
 * The port a check's form shows before one is typed, and what becomes of
 * the port when the check's type is changed.
 *
 * The form showed 443 for a TCP check and 53 for a UDP one while the check
 * itself carried no port, and a check with no port was run on port 0: it
 * failed for ever beside a field reading 443. The number shown is now the
 * number kept. The Rust side falls back to the same two, for checks made
 * before this (`corelens_probe::engine::default_port`).
 */
import type { ProbeKind } from '../types/domain';

/** What the Port field shows for a type until another is typed. */
export function defaultPort(kind: ProbeKind): number {
  if (kind === 'http') return 80;
  if (kind === 'udp') return 53;
  return 443;
}

/** Types whose check is a port on the target and nothing else. */
const NEEDS_A_PORT = new Set<ProbeKind>(['tcp', 'udp']);

/**
 * The port to keep when the type goes from `from` to `to`.
 *
 * A port somebody typed is theirs and is kept. One that was only ever the
 * old type's default follows the type: the new type's default where the
 * check needs a port, none where its engine has its own (HTTP, HTTPS) or it
 * has no port at all.
 */
export function portForKind(from: ProbeKind, port: number | null | undefined, to: ProbeKind): number | null {
  const untouched = port == null || port === defaultPort(from);
  if (!untouched) return port;
  return NEEDS_A_PORT.has(to) ? defaultPort(to) : null;
}
