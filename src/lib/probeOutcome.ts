/**
 * What one result of a check means for the count the interface keeps
 * beside it ("1 of 3 missed").
 *
 * The engine's own rule, said once more on this side, where the same count
 * is kept from the results as they arrive: a device that answered has
 * missed nothing, including one that answered and had restarted; a check
 * that could not be made at all asked nothing of the device, and is not a
 * miss either; anything else is one more.
 */
import type { ProbeResultDto } from './ipc';

/** As the engine names them; a plain string where a result is held loosely. */
type Outcome = ProbeResultDto['outcome'] | (string & {});

/** The device answered. */
export function answered(outcome: Outcome): boolean {
  return outcome === 'success' || outcome === 'restarted';
}

/** Something was asked of the device. */
export function wasCarriedOut(outcome: Outcome): boolean {
  return outcome !== 'invalid_target' && outcome !== 'not_run';
}

/** The run of misses after this result, given the run before it. */
export function missesAfter(outcome: Outcome, before: number): number {
  if (answered(outcome) || !wasCarriedOut(outcome)) return 0;
  return before + 1;
}
