import crypto from 'crypto';
import { BaseProvider, providerHttpError, type CompletionOptions, type ProviderHttpError } from './base.js';
import type { Platform, ChatMessage, ChatCompletionResponse, ChatCompletionChunk } from '../types.js';
import { extractVscodeBlackboxTokens } from '../utils/vscode-extractor.js';

const BLACKBOX_API_BASE = 'https://api.blackbox.ai/v1';
const BLACKBOX_FREE_BASE = 'https://oi-vscode-server-985058387028.europe-west1.run.app';

function randomId(len = 24): string {
  return crypto.randomBytes(len).toString('hex');
}

function isPlaceholderKey(apiKey: string | undefined): boolean {
  const trimmed = apiKey?.trim();
  return !trimmed || trimmed === 'xxx' || trimmed === 'minimax-no-key-required';
}

function getEnvApiKey(): string | undefined {
  const value = process.env.BLACKBOX_API_KEY
    || process.env.BLACKBOX_CUSTOMER_ID
    || process.env.BLACKBOX_CUSTOMER_TOKEN;
  return isPlaceholderKey(value) ? undefined : value?.trim();
}

function normalizeModelId(modelId: string): string {
  const lower = modelId.toLowerCase();
  if (lower === 'kimi-k2.6' || lower === 'kimi') {
    return 'moonshotai/kimi-k2.6';
  }
  if (lower === 'gpt-5.5' || lower === '5.5') {
    return 'minimax-m2.7';
  }
  if (lower === 'gpt-5.4' || lower === '5.4') {
    return 'custom/blackbox-base-2';
  }
  if (lower === 'gpt-5.4-mini' || lower === '5.4 mini' || lower === '5.4-mini') {
    return 'gpt-4o-mini';
  }
  return modelId;
}

function isMinimaxModel(modelId: string): boolean {
  const lower = modelId.toLowerCase();
  return lower === 'minimax-m2'
    || lower === 'minimax-m2.7'
    || lower === 'minimax-m2.5'
    || lower === 'minimax-free'
    || lower === 'openrouter/minimax-m2-thinking';
}

function isKimiModel(modelId: string): boolean {
  return modelId.toLowerCase() === 'moonshotai/kimi-k2.6';
}

function providerAuthError(message: string): ProviderHttpError {
  const err = new Error(message) as ProviderHttpError;
  err.status = 401;
  return err;
}

export class BlackboxProvider extends BaseProvider {
  readonly platform: Platform = 'blackbox';
  readonly name = 'Blackbox AI';
  keyless = true;

  private async postChat(apiKey: string | undefined, body: Record<string, unknown>, timeoutMs = 30000): Promise<Response> {
    const modelId = normalizeModelId(body.model as string);
    const isMinimax = isMinimaxModel(modelId);
    const isKimi = isKimiModel(modelId);
    body.model = modelId;

    // Auto-extract VS Code tokens if no explicit apiKey is provided
    let effectiveApiKey = isPlaceholderKey(apiKey) ? undefined : apiKey?.trim();
    if (!effectiveApiKey) {
      effectiveApiKey = getEnvApiKey();
    }
    if (!effectiveApiKey) {
      const tokens = await extractVscodeBlackboxTokens();
      if (tokens.customerId) {
        effectiveApiKey = tokens.customerId;
      } else if (tokens.apiKey) {
        effectiveApiKey = tokens.apiKey;
      }
    }
    
    // If we still don't have a key but it's minimax, fall back to the bypass key
    if (!effectiveApiKey && isMinimax) {
      effectiveApiKey = 'minimax-no-key-required';
    }

    if (!effectiveApiKey && isKimi) {
      throw providerAuthError(
        'Blackbox Kimi K2.6 requires a saved Blackbox customer token. Open the Blackbox extension/sidebar and send a normal chat message first, or pass a valid Blackbox API key.',
      );
    }

    if (effectiveApiKey && effectiveApiKey.startsWith('sk-')) {
      // Authenticated requests go to the standard API
      return this.fetchWithTimeout(`${BLACKBOX_API_BASE}/chat/completions`, {
        method: 'POST',
        headers: {
          'Authorization': `Bearer ${effectiveApiKey}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify(body),
      }, timeoutMs);
    } else {
      // Keyless (free) requests go to the VS Code server proxy
      const userId = randomId(16);
      const isCustomerToken = effectiveApiKey && effectiveApiKey !== 'minimax-no-key-required' && effectiveApiKey.length > 10;
      
      const headers: Record<string, string> = {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${isMinimax ? 'minimax-no-key-required' : 'xxx'}`,
        'userId': userId,
        'version': '1.1'
      };

      if (isCustomerToken) {
        headers['customerId'] = effectiveApiKey as string;
      }

      return this.fetchWithTimeout(`${BLACKBOX_FREE_BASE}/chat/completions`, {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
      }, timeoutMs);
    }
  }

  async chatCompletion(
    apiKey: string,
    messages: ChatMessage[],
    modelId: string,
    options?: CompletionOptions,
  ): Promise<ChatCompletionResponse> {
    const body = {
      model: modelId,
      messages,
      temperature: options?.temperature,
      max_tokens: options?.max_tokens,
      top_p: options?.top_p,
      tools: options?.tools,
      tool_choice: options?.tool_choice,
      parallel_tool_calls: options?.parallel_tool_calls,
      stream: false,
    };

    const res = await this.postChat(apiKey, body, options?.timeoutMs ?? 30000);
    
    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw providerHttpError(res, `${this.name} API error ${res.status}: ${(err as any)?.error?.message ?? (err as any)?.message ?? res.statusText}`);
    }
    
    const data = await res.json() as any;
    data._routed_via = { platform: this.platform, model: modelId };
    return data as ChatCompletionResponse;
  }

  async *streamChatCompletion(
    apiKey: string,
    messages: ChatMessage[],
    modelId: string,
    options?: CompletionOptions,
  ): AsyncGenerator<ChatCompletionChunk> {
    const body = {
      model: modelId,
      messages,
      temperature: options?.temperature,
      max_tokens: options?.max_tokens,
      top_p: options?.top_p,
      tools: options?.tools,
      tool_choice: options?.tool_choice,
      parallel_tool_calls: options?.parallel_tool_calls,
      stream: true,
    };

    const res = await this.postChat(apiKey, body, options?.timeoutMs ?? 30000);

    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw providerHttpError(res, `${this.name} API error ${res.status}: ${(err as any)?.error?.message ?? (err as any)?.message ?? res.statusText}`);
    }

    yield* this.readSseStream(res, 110000);
  }

  async validateKey(apiKey: string): Promise<boolean> {
    try {
      const res = await this.postChat(apiKey, {
        model: 'gpt-4o-mini',
        messages: [{ role: 'user', content: 'hi' }],
        max_tokens: 4,
      }, 15000);
      return res.ok;
    } catch {
      return false;
    }
  }
}
