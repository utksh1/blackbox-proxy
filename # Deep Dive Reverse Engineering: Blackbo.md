# Deep Dive Reverse Engineering: Blackbox Local Autonomous Agent (ACP)

This document compiles the technical findings and architectural details gathered during our reverse engineering research of the **Blackbox Agent - Coding Copilot** (`blackboxapp.blackboxagent`) extension, focusing **exclusively on the Local Autonomous Agent (ACP)** and the models that operate within it for free.

---

## 1. ACP Execution Pipeline

The Local Autonomous Agent (ACP) is the subsystem responsible for executing workspace commands and terminal automation. 

Located within the main bundle `dist/extension.js`, the ACP initialization sequence (`Raa()`) acts as the gatekeeper for local tool execution. Before spawning the agent process, it verifies authentication via a local storage key.

---

## 2. Model Behaviors in the ACP Agent

We tested the specific models requested to determine how they interact with the ACP execution pipeline. 

### Minimax M2.7
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

Because of this bypass, **Minimax M2.7** runs natively as a local autonomous agent without any prior login or token required. The system injects `"minimax-no-key-required"` as the API key, skipping the standard auth checks completely.

### Kimi K2.6
**Status: ✅ Working (Requires Background Token)**

Unlike Minimax, Kimi K2.6 does **not** have a hardcoded bypass in the initialization function `Raa()`. When you try to run the ACP agent using Kimi, the system enforces the standard API key check:

```javascript
m = cH().getApiKeyFromStorage("blackbox")
```

If it does not find a key, the ACP agent will throw an error and fail to start.

**However, Kimi still works for free.** This is because of the **"saved background token"**. 
If you open the Blackbox extension sidebar and perform any action (like sending a normal chat message), the extension silently communicates with the Blackbox servers, generates a free-tier session token, and saves it to your IDE's local storage under the `"blackbox"` key. 

Once this background token is saved, the ACP agent successfully retrieves it during initialization. Kimi uses this saved token to authorize itself and successfully execute local tools.

---

## Summary

* **Minimax M2.7:** Skips the local storage API key check completely. You can install the extension and use it as an ACP agent immediately.
* **Kimi K2.6:** Enforces the API key check. You must initialize the extension first (e.g., by using the standard chat sidebar) to generate the "saved background token" before the ACP agent will work.
