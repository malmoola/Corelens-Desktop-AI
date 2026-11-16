import { describe, expect, it } from 'vitest';
import { modelChoices } from './aiModels';

describe('the model list', () => {
  it('offers every other model while one is chosen', () => {
    const ids = modelChoices('claude-opus-5-5', [], ['claude-opus-5-5', 'claude-sonnet-5-5', 'claude-haiku-5-5']).map((m) => m.id);
    expect(ids).toEqual(['claude-opus-5-5', 'claude-sonnet-5-5', 'claude-haiku-5-5']);
  });

  it('puts what the provider listed first, keeps its notes, and adds the chosen one if it was not listed', () => {
    const got = modelChoices('my-own-model', [{ id: 'qwen3', label: '5 GB', tools: true }, { id: 'tiny', label: '', tools: false }], ['qwen3', 'llama3.1']);
    expect(got.map((m) => m.id)).toEqual(['qwen3', 'tiny', 'llama3.1', 'my-own-model']);
    expect(got[0]).toEqual({ id: 'qwen3', label: '5 GB', tools: true });
    expect(got[1]!.tools).toBe(false);
  });

  it('has no blank entry', () => {
    expect(modelChoices('', [{ id: ' ', label: '', tools: null }], []).length).toBe(0);
  });
});
