import { describe, expect, it } from 'vitest';

import { readAiSettings } from './aiSettings';

const full = {
  chatEnabled: false,
  provider: 'anthropic',
  models: { anthropic: 'model-a', openai: 'model-b', ollama: 'model-c' },
  openaiBaseUrl: 'https://api.example.test/v1',
  openaiStyle: 'responses',
  ollamaBaseUrl: 'http://localhost:11434',
  maxToolCalls: 15,
  toolOutputChars: 12000,
  chatRetentionDays: 90,
  auditRetentionDays: 90,
  proposalTimeoutMinutes: 10,
  keys: { anthropic: false },
};

describe('readAiSettings', () => {
  it('passes a complete reply through unchanged', () => {
    expect(readAiSettings(full)).toEqual(full);
  });

  it('turns a reply without models or keys into an error the block can show, not a crash of Settings', () => {
    for (const bad of [[], {}, { ...full, models: undefined }, { ...full, keys: null }, { ...full, provider: 'other' }, 'x', 7]) {
      expect(() => readAiSettings(bad)).toThrow(/AI settings/);
    }
  });

  it('leaves the browser preview, which has no backend, as no settings', () => {
    expect(readAiSettings(null)).toBeNull();
  });
});
