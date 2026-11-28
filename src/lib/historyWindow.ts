/**
 * How much of a chosen window a check's stored history really covers.
 *
 * The results come newest first up to a limit. Asked for 24 hours at the
 * default interval there are over seventeen thousand; the view asked for
 * two thousand without saying so, showed the last three hours as "24
 * hours" and worked out "% up" from those. It now asks for as many as the
 * backend gives, and where even that is not the whole window it says from
 * when the results shown begin.
 */

/** The most results the backend returns for one check (`probe_history`). */
export const HISTORY_LIMIT = 20_000;

export interface Coverage {
  /** As many came back as were asked for: older ones in the window were left out. */
  cut: boolean;
  /** When the results shown begin, or null with none. */
  fromMs: number | null;
}

export function coverage(samples: { timestampMs: number }[], limit: number): Coverage {
  if (samples.length === 0) return { cut: false, fromMs: null };
  let fromMs = samples[0]!.timestampMs;
  for (const s of samples) if (s.timestampMs < fromMs) fromMs = s.timestampMs;
  return { cut: samples.length >= limit, fromMs };
}
