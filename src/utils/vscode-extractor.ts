import os from 'os';
import path from 'path';
import fs from 'fs';
import { execFileSync } from 'child_process';

/**
 * Utility to extract Blackbox tokens from VS Code-family global state SQLite databases.
 */

type BlackboxTokens = { customerId: string | null; apiKey: string | null };

// Cache the extracted tokens to avoid hitting the SQLite database on every request
let cachedTokens: BlackboxTokens | null = null;
let lastExtractedTime = 0;
const CACHE_TTL_MS = 60 * 1000; // 1 minute
const SQLITE_MAX_BUFFER = 20 * 1024 * 1024;
const BLACKBOX_STORAGE_KEYS = ['Blackboxapp.blackboxagent', 'Blackboxapp.blackbox'];
const API_KEY_FIELDS = new Set(['apikey', 'fallback_apikey']);
const CUSTOMER_ID_FIELDS = new Set(['customerid', 'subscriptionid']);

function getVscodeStateDbPaths(): string[] {
  const platform = os.platform();
  const home = os.homedir();

  if (platform === 'darwin') {
    return [
      path.join(home, 'Library', 'Application Support', 'Code', 'User', 'globalStorage', 'state.vscdb'),
      path.join(home, 'Library', 'Application Support', 'Cursor', 'User', 'globalStorage', 'state.vscdb'),
      path.join(home, 'Library', 'Application Support', 'Code - Insiders', 'User', 'globalStorage', 'state.vscdb'),
      path.join(home, 'Library', 'Application Support', 'VSCodium', 'User', 'globalStorage', 'state.vscdb'),
    ];
  }

  if (platform === 'win32') {
    const appData = process.env.APPDATA || path.join(home, 'AppData', 'Roaming');
    return [
      path.join(appData, 'Code', 'User', 'globalStorage', 'state.vscdb'),
      path.join(appData, 'Cursor', 'User', 'globalStorage', 'state.vscdb'),
      path.join(appData, 'Code - Insiders', 'User', 'globalStorage', 'state.vscdb'),
      path.join(appData, 'VSCodium', 'User', 'globalStorage', 'state.vscdb'),
    ];
  }

  if (platform === 'linux') {
    return [
      path.join(home, '.config', 'Code', 'User', 'globalStorage', 'state.vscdb'),
      path.join(home, '.config', 'Cursor', 'User', 'globalStorage', 'state.vscdb'),
      path.join(home, '.config', 'Code - Insiders', 'User', 'globalStorage', 'state.vscdb'),
      path.join(home, '.config', 'VSCodium', 'User', 'globalStorage', 'state.vscdb'),
    ];
  }

  return [];
}

function extractTokensFromObject(value: unknown): BlackboxTokens {
  const tokens: BlackboxTokens = { customerId: null, apiKey: null };
  const queue: unknown[] = [value];

  while (queue.length > 0 && (!tokens.customerId || !tokens.apiKey)) {
    const current = queue.shift();
    if (!current || typeof current !== 'object') continue;

    for (const [key, child] of Object.entries(current as Record<string, unknown>)) {
      const normalizedKey = key.toLowerCase();

      if (!tokens.customerId && CUSTOMER_ID_FIELDS.has(normalizedKey) && typeof child === 'string' && child.trim()) {
        tokens.customerId = child.trim();
      }

      if (!tokens.apiKey && API_KEY_FIELDS.has(normalizedKey) && typeof child === 'string' && child.trim()) {
        tokens.apiKey = child.trim();
      }

      if (child && typeof child === 'object') {
        queue.push(child);
      }
    }
  }

  return tokens;
}

function hasToken(tokens: BlackboxTokens): boolean {
  return Boolean(tokens.customerId || tokens.apiKey);
}

function querySqliteValue(dbPath: string, key: string): string | null {
  const escapedKey = key.replaceAll("'", "''");
  const query = "SELECT value FROM ItemTable WHERE key = '" + escapedKey + "';";
  const output = execFileSync('sqlite3', [dbPath, query], {
    encoding: 'utf-8',
    stdio: ['ignore', 'pipe', 'ignore'],
    timeout: 2000,
    maxBuffer: SQLITE_MAX_BUFFER,
  }).trim();

  return output || null;
}

function extractTokensFromRawDb(dbPath: string): BlackboxTokens {
  const content = fs.readFileSync(dbPath).toString('utf-8');
  const customerIdMatch = content.match(/"customerId"\s*:\s*"([^"]+)"/);
  const apiKeyMatch = content.match(/"(?:apiKey|fallback_apiKey)"\s*:\s*"([^"]+)"/);

  return {
    customerId: customerIdMatch ? customerIdMatch[1] : null,
    apiKey: apiKeyMatch ? apiKeyMatch[1] : null,
  };
}

export async function extractVscodeBlackboxTokens(): Promise<BlackboxTokens> {
  // Return cached version if valid
  if (cachedTokens && Date.now() - lastExtractedTime < CACHE_TTL_MS) {
    return cachedTokens;
  }

  const dbPaths = getVscodeStateDbPaths().filter(dbPath => fs.existsSync(dbPath));
  if (dbPaths.length === 0) {
    return { customerId: null, apiKey: null };
  }

  // Attempt 1: Use sqlite3 CLI if available. Blackbox's agent state can be
  // several MB because it embeds marketplace data, so keep maxBuffer explicit.
  for (const dbPath of dbPaths) {
    for (const key of BLACKBOX_STORAGE_KEYS) {
      try {
        const output = querySqliteValue(dbPath, key);
        if (!output) continue;

        const tokens = extractTokensFromObject(JSON.parse(output));
        if (hasToken(tokens)) {
          cachedTokens = tokens;
          lastExtractedTime = Date.now();
          return cachedTokens;
        }
      } catch {
        // sqlite3 failed, JSON parsing failed, or this key had no tokens.
      }
    }
  }

  // Attempt 2: Read the raw database file and use regex. This works because
  // SQLite usually stores these JSON blobs contiguously.
  for (const dbPath of dbPaths) {
    try {
      const tokens = extractTokensFromRawDb(dbPath);
      if (hasToken(tokens)) {
        cachedTokens = tokens;
        lastExtractedTime = Date.now();
        return cachedTokens;
      }
    } catch {
      // Silently ignore
    }
  }

  // If both methods fail
  cachedTokens = { customerId: null, apiKey: null };
  lastExtractedTime = Date.now();
  return cachedTokens;
}
