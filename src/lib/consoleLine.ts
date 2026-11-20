/**
 * A serial console's line, and which port to open it on.
 *
 * The port is found rather than asked for. `console_ports` lists the
 * machine's ports with known console adapters first; `pickPort` chooses
 * from that list, and prefers a port that has just appeared — the cable
 * that was plugged in while the dialog was open is the one meant.
 */
import type { ConsoleLine, ConsolePort } from './ipc';

/** The speeds a console runs at; the backend refuses any other. */
export const CONSOLE_SPEEDS = [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

/** Data bits, parity and stop bits, the way terminals write them. */
export const FRAMINGS = ['8N1', '8E1', '8O1', '8N2', '7E1', '7O1', '7N1', '7E2'];

/** Whether this is a framing such as `8N1`. */
export function framing(raw: string | undefined): boolean {
  return raw !== undefined && FRAMINGS.includes(raw);
}

/** A line from the remembered settings. */
export function lineFrom(speed: number, framed: string, flow: ConsoleLine['flow']): ConsoleLine {
  const f = framing(framed) ? framed : '8N1';
  return {
    speed: CONSOLE_SPEEDS.includes(speed) ? speed : 9600,
    dataBits: f[0] === '7' ? 7 : 8,
    parity: f[1] === 'E' ? 'even' : f[1] === 'O' ? 'odd' : 'none',
    stopBits: f[2] === '2' ? 2 : 1,
    flow,
  };
}

/** `9600 8N1`, as a tab's title says it. */
export function describeLine(line: ConsoleLine): string {
  const parity = line.parity === 'even' ? 'E' : line.parity === 'odd' ? 'O' : 'N';
  return `${line.speed} ${line.dataBits}${parity}${line.stopBits}`;
}

/** Why a port was chosen, for the dialog to say. */
export type Picked =
  | { name: string; why: 'plugged' | 'console' | 'usb' | 'only' }
  | { name: null; why: 'none' | 'unsure' };

/**
 * The port to open.
 *
 * `before` is the list as it was the last time it was looked at, or
 * undefined the first time. A port in `now` that was not in `before` has
 * just been plugged in, and wins. Otherwise the first known console
 * adapter, then the only USB port; a choice among several unknown ports is
 * not made for anyone.
 */
export function pickPort(now: ConsolePort[], before?: ConsolePort[]): Picked {
  if (before) {
    const had = new Set(before.map((p) => p.name));
    const fresh = now.filter((p) => !had.has(p.name) && p.kind !== 'bluetooth');
    const plugged = fresh.find((p) => p.console) ?? fresh[0];
    if (plugged) return { name: plugged.name, why: 'plugged' };
  }
  const cable = now.find((p) => p.console);
  if (cable) return { name: cable.name, why: 'console' };
  const usb = now.filter((p) => p.kind === 'usb');
  if (usb.length === 1 && usb[0]) return { name: usb[0].name, why: 'usb' };
  const usable = now.filter((p) => p.kind !== 'bluetooth');
  if (usable.length === 1 && usable[0]) return { name: usable[0].name, why: 'only' };
  return usable.length === 0 ? { name: null, why: 'none' } : { name: null, why: 'unsure' };
}

/** A port's name as it can stand in a log file's name: `/dev/ttyUSB0`
 *  becomes `ttyUSB0`; `COM3` stays. */
export function logAddress(port: string): string {
  return port.replace(/^.*[\\/]/, '');
}
