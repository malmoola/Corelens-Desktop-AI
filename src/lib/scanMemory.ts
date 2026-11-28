/**
 * What a discovery form remembers of how a run may log in, as settings.
 *
 * The seed, the subnets and the login were kept with the project and came
 * back when the form was put on the screen again; the kinds of device the
 * run may log in to, the engine, the transport, hopping through and the
 * wait for a second factor were not, and came back as their defaults:
 * routers and firewalls unticked for a customer's network were ticked
 * again after a look at another tab. These are the same kind of thing and
 * are kept the same way. A setting is one string.
 */

/** The kinds of device ticked, as a setting: a list, so that none at all
 *  is told from nothing kept. */
export function loginKindsToSetting(kinds: readonly string[]): string {
  return JSON.stringify([...kinds]);
}

/**
 * The kinds kept, or null where nothing was (the form's own default then).
 * Only kinds the form still offers; anything else in the setting is dropped.
 */
export function loginKindsFromSetting<T extends string>(value: string | null | undefined, offered: readonly T[]): T[] | null {
  if (!value) return null;
  try {
    const kept: unknown = JSON.parse(value);
    if (!Array.isArray(kept)) return null;
    return offered.filter((k) => kept.includes(k));
  } catch {
    return null;
  }
}

/** A kept choice, if it is one of those there are. */
export function oneOf<T extends string>(value: string | null | undefined, choices: readonly T[]): T | null {
  return choices.includes(value as T) ? (value as T) : null;
}
