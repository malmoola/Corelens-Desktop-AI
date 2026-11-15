/**
 * The AI settings as the page receives them. The backend always sends the
 * whole shape; a reply without it (a stand-in backend, a mismatched build)
 * becomes an error the AI block shows, instead of an undefined read that
 * takes the whole Settings screen down with it.
 */
import { t } from '../i18n';
import type { AiProviderId, AiSettingsDto } from './ipc';

const PROVIDERS: readonly AiProviderId[] = ['anthropic', 'openai', 'ollama'];

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

export function readAiSettings(v: unknown): AiSettingsDto | null {
  if (v === null || v === undefined) return null;
  if (
    !isRecord(v) ||
    !PROVIDERS.includes(v.provider as AiProviderId) ||
    !isRecord(v.models) ||
    !PROVIDERS.every((p) => typeof (v.models as Record<string, unknown>)[p] === 'string') ||
    !isRecord(v.keys)
  ) {
    throw new Error(t('aiSettings.unreadable'));
  }
  return v as unknown as AiSettingsDto;
}
