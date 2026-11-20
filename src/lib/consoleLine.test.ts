import { describe, expect, it } from 'vitest';

import { describeLine, lineFrom, logAddress, pickPort } from './consoleLine';
import type { ConsolePort } from './ipc';

const port = (name: string, kind: ConsolePort['kind'], console = false): ConsolePort =>
  ({ name, kind, description: 'test port', console });

describe('which console port is opened', () => {
  it('a port that has just appeared is the cable that was just plugged in', () => {
    const before = [port('COM1', 'other'), port('COM3', 'usb', true)];
    const now = [...before, port('COM9', 'usb')];
    expect(pickPort(now, before)).toEqual({ name: 'COM9', why: 'plugged' });
  });

  it('a Bluetooth link that appears is not mistaken for a cable', () => {
    const before = [port('COM3', 'usb', true)];
    expect(pickPort([...before, port('COM8', 'bluetooth')], before)).toEqual({ name: 'COM3', why: 'console' });
  });

  it('otherwise the first known console adapter', () => {
    expect(pickPort([port('COM3', 'usb', true), port('COM5', 'usb', true), port('COM1', 'other')]))
      .toEqual({ name: 'COM3', why: 'console' });
  });

  it('then the only USB port, then the only port at all', () => {
    expect(pickPort([port('COM6', 'usb'), port('COM1', 'other')])).toEqual({ name: 'COM6', why: 'usb' });
    expect(pickPort([port('COM1', 'other'), port('COM4', 'bluetooth')])).toEqual({ name: 'COM1', why: 'only' });
  });

  it('does not guess among several unknown ports, and says when there are none', () => {
    expect(pickPort([port('COM1', 'other'), port('COM2', 'other')])).toEqual({ name: null, why: 'unsure' });
    expect(pickPort([port('COM4', 'bluetooth')])).toEqual({ name: null, why: 'none' });
    expect(pickPort([])).toEqual({ name: null, why: 'none' });
  });
});

describe('a console line', () => {
  it('reads framing the way terminals write it', () => {
    expect(lineFrom(9600, '8N1', 'none')).toEqual({ speed: 9600, dataBits: 8, parity: 'none', stopBits: 1, flow: 'none' });
    expect(describeLine(lineFrom(115200, '7E2', 'hardware'))).toBe('115200 7E2');
  });

  it('falls back to 9600 8N1 for anything it does not know', () => {
    expect(describeLine(lineFrom(9601, 'X', 'none'))).toBe('9600 8N1');
  });

  it('names a log by the port, never by a path', () => {
    expect(logAddress('/dev/ttyUSB0')).toBe('ttyUSB0');
    expect(logAddress('COM3')).toBe('COM3');
    expect(logAddress('\\\\.\\COM12')).toBe('COM12');
  });
});
