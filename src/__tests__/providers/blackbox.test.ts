import { beforeEach, describe, expect, it, vi } from 'vitest';
import { BlackboxProvider } from '../../providers/blackbox.js';
import { extractVscodeBlackboxTokens } from '../../utils/vscode-extractor.js';

vi.mock('../../utils/vscode-extractor.js', () => ({
  extractVscodeBlackboxTokens: vi.fn(),
}));

const mockedExtractVscodeBlackboxTokens = vi.mocked(extractVscodeBlackboxTokens);

function jsonResponse(body: any, status = 200, headers: Record<string, string> = {}) {
  return {
    ok: status < 400,
    status,
    statusText: status < 400 ? 'OK' : 'Error',
    headers: new Headers({
      'content-type': 'application/json',
      ...headers,
    }),
    json: async () => body,
  } as unknown as Response;
}

const okBody = {
  id: 'gen-1',
  object: 'chat.completion',
  created: 1,
  model: 'test-model',
  choices: [
    {
      index: 0,
      message: { role: 'assistant', content: 'ok' },
      finish_reason: 'stop' as const,
    },
  ],
  usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
};

describe('BlackboxProvider', () => {
  let provider: BlackboxProvider;
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    delete process.env.BLACKBOX_API_KEY;
    delete process.env.BLACKBOX_CUSTOMER_ID;
    delete process.env.BLACKBOX_CUSTOMER_TOKEN;
    provider = new BlackboxProvider();
    mockedExtractVscodeBlackboxTokens.mockReset();
    mockedExtractVscodeBlackboxTokens.mockResolvedValue({ customerId: null, apiKey: null });
    fetchMock = vi.fn(async () => jsonResponse(okBody));
    vi.stubGlobal('fetch', fetchMock);
  });

  it('has the expected provider metadata', () => {
    expect(provider.platform).toBe('blackbox');
    expect(provider.name).toBe('Blackbox AI');
    expect(provider.keyless).toBe(true);
  });

  it('routes anonymous free requests through the VS Code proxy', async () => {
    await provider.chatCompletion('', [{ role: 'user', content: 'hi' }], 'gpt-4o-mini');

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('https://oi-vscode-server-985058387028.europe-west1.run.app/chat/completions');
    expect(init.headers).toMatchObject({
      Authorization: 'Bearer xxx',
      version: '1.1',
    });
    expect(init.headers.userId).toMatch(/^[a-f0-9]{32}$/);
    expect(JSON.parse(init.body).model).toBe('gpt-4o-mini');
  });

  it('maps Minimax aliases to the upstream OpenRouter model and bypass key', async () => {
    await provider.chatCompletion('', [{ role: 'user', content: 'hi' }], 'minimax-m2.7');

    const [, init] = fetchMock.mock.calls[0];
    expect(init.headers.Authorization).toBe('Bearer minimax-no-key-required');
    expect(JSON.parse(init.body).model).toBe('openrouter/minimax-m2-thinking');
  });

  it('requires a real token for Kimi aliases before calling upstream', async () => {
    await expect(
      provider.chatCompletion('', [{ role: 'user', content: 'hi' }], 'KIMI-K2.6'),
    ).rejects.toMatchObject({
      status: 401,
      message: expect.stringContaining('requires a saved Blackbox customer token'),
    });

    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('normalizes Kimi aliases and forwards an extracted customer token', async () => {
    mockedExtractVscodeBlackboxTokens.mockResolvedValue({
      customerId: 'customer-token-123456',
      apiKey: null,
    });

    await provider.chatCompletion('', [{ role: 'user', content: 'hi' }], 'Kimi');

    const [, init] = fetchMock.mock.calls[0];
    expect(init.headers).toMatchObject({
      Authorization: 'Bearer xxx',
      customerId: 'customer-token-123456',
    });
    expect(JSON.parse(init.body).model).toBe('moonshotai/kimi-k2.6');
  });

  it('uses an env customer token for Kimi when storage is empty', async () => {
    process.env.BLACKBOX_CUSTOMER_ID = 'customer-token-from-env';

    await provider.chatCompletion('', [{ role: 'user', content: 'hi' }], 'kimi-k2.6');

    expect(mockedExtractVscodeBlackboxTokens).not.toHaveBeenCalled();
    const [, init] = fetchMock.mock.calls[0];
    expect(init.headers).toMatchObject({
      Authorization: 'Bearer xxx',
      customerId: 'customer-token-from-env',
    });
    expect(JSON.parse(init.body).model).toBe('moonshotai/kimi-k2.6');
  });

  it('uses the standard API for explicit sk-style keys', async () => {
    await provider.chatCompletion('sk-test-key', [{ role: 'user', content: 'hi' }], 'gpt-4o-mini');

    expect(mockedExtractVscodeBlackboxTokens).not.toHaveBeenCalled();
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('https://api.blackbox.ai/v1/chat/completions');
    expect(init.headers).toMatchObject({
      Authorization: 'Bearer sk-test-key',
    });
  });

  it('uses env sk-style keys for the standard API', async () => {
    process.env.BLACKBOX_API_KEY = 'sk-env-test-key';

    await provider.chatCompletion('', [{ role: 'user', content: 'hi' }], 'kimi-k2.6');

    expect(mockedExtractVscodeBlackboxTokens).not.toHaveBeenCalled();
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('https://api.blackbox.ai/v1/chat/completions');
    expect(init.headers).toMatchObject({
      Authorization: 'Bearer sk-env-test-key',
    });
    expect(JSON.parse(init.body).model).toBe('moonshotai/kimi-k2.6');
  });

  it('propagates non-auth upstream errors', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse({ error: { message: 'server error' } }, 500));

    await expect(
      provider.chatCompletion('', [{ role: 'user', content: 'hi' }], 'gpt-4o-mini'),
    ).rejects.toThrow('Blackbox AI API error 500: server error');
  });
});
