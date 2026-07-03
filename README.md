# Blackbox AI Proxy

A lightning-fast, lightweight local proxy server written in Rust (`axum`) that provides a standard OpenAI-compatible API interface to the internal Blackbox AI VS Code extension backend. 

This proxy allows you to use top-tier models (like **Minimax-m2.7** and **Kimi-k2.6**) entirely for free by reverse-engineering the authentication bypasses and token mechanisms used by the official Blackbox VS Code extension.

## Features
- **OpenAI Compatible**: Fully supports `/chat/completions` and streaming responses. Drop-in replacement for OpenAI API clients.
- **Zero Configuration**: Automatically scans your VS Code/Cursor/VSCodium `globalStorage` SQLite databases to extract your logged-in Blackbox `customerId` and `apiKey`.
- **Smart Routing**: Dynamically routes requests to `BLACKBOX_FREE_BASE` or `BLACKBOX_PRO_BASE` depending on the presence of an active customer token.
- **Ultra-Lightweight**: Written in Rust. Consumes ~10MB RAM in release mode with instantaneous startup times (no Node.js or `node_modules` required).

## Installation & Usage

1. Clone the repository.
2. Build and run the proxy using Cargo:
   ```bash
   cargo run --release
   ```
   The proxy will start listening on `http://0.0.0.0:8080`.

3. Send a request using the default proxy key (`xyz`):
   ```bash
   curl -X POST http://localhost:8080/chat/completions \
     -H "Content-Type: application/json" \
     -H "Authorization: Bearer xyz" \
     -d '{
       "model": "minimax-m2.7", 
       "messages": [{"role": "user", "content": "Hello!"}]
     }'
   ```

*(Note: You can override the default proxy key by setting the `PROXY_API_KEY` environment variable).*

---

## Deep Dive Reverse Engineering: Blackbox Local Autonomous Agent (ACP)

This project was built on technical findings and architectural details gathered by reverse engineering the **Blackbox Agent - Coding Copilot** (`blackboxapp.blackboxagent`) extension, focusing specifically on the models that operate within it for free.

### 1. The ACP Execution Pipeline

The Local Autonomous Agent (ACP) is the subsystem responsible for executing workspace commands and terminal automation in Blackbox. Located within the main bundle `dist/extension.js`, the ACP initialization sequence (`Raa()`) acts as the gatekeeper for local tool execution. Before spawning the agent process, it verifies authentication via a local storage key.

### 2. Model Behaviors & Bypasses

#### Minimax M2.7
**Status: ✅ Fully Working (Hardcoded Bypass)**

The extension source code contains an explicit bypass whitelist for MiniMax models. When the ACP agent initializes, it checks the model name:

```javascript
let h = d?.toLowerCase()?.includes("minimax-free") 
     || d?.toLowerCase()?.includes("minimax-m2.5") 
     || d?.toLowerCase()?.includes("minimax-m2.7");

if (h) {
    m = "minimax-no-key-required";
    console.log("[ACP] MiniMax model detected - skipping API key requirement");
}
```

Because of this bypass, **Minimax M2.7** runs natively as a local autonomous agent without any prior login or token required. The system injects `"minimax-no-key-required"` as the API key, skipping the standard auth checks completely. **Our proxy automatically replicates this bypass.**

#### Kimi K2.6
**Status: ✅ Working (Requires Background Token)**

Unlike Minimax, Kimi K2.6 does **not** have a hardcoded bypass in the initialization function `Raa()`. When you try to run the ACP agent using Kimi, the system enforces the standard API key check:

```javascript
m = cH().getApiKeyFromStorage("blackbox")
```

If it does not find a key, the agent fails to start. **However, Kimi still works for free** because of a **saved background token**. 

If you open the Blackbox extension sidebar in VS Code and perform any action (like sending a normal chat message), the extension silently communicates with the Blackbox servers, generates a free-tier session token, and saves it to your IDE's local SQLite storage under the `"blackbox"` key. 

**Our proxy automatically reads this token from your SQLite database** and injects it into your API requests. Kimi uses this saved token to authorize itself and successfully execute local tools.
