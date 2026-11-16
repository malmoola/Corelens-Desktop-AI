/**
 * What the model list in Settings → AI offers.
 */
import type { AiModel } from './ipc';

/** Every model the provider listed or is known to offer, and the one chosen,
 *  in a list that shows them all whatever is chosen. "Another model…" takes
 *  an ID typed by hand. */
export function modelChoices(chosen: string, listed: readonly AiModel[], suggested: readonly string[]): AiModel[] {
  const out: AiModel[] = [];
  const add = (m: AiModel) => {
    if (m.id.trim() && !out.some((o) => o.id === m.id)) out.push(m);
  };
  listed.forEach(add);
  suggested.forEach((id) => add({ id, label: '', tools: null }));
  if (chosen.trim()) add({ id: chosen, label: '', tools: null });
  return out;
}
