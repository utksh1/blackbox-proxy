import express from 'express';
import cors from 'cors';
import swaggerUi from 'swagger-ui-express';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import yaml from 'yaml';
import { BlackboxProvider } from './providers/blackbox.js';
import type { CompletionOptions } from './providers/base.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const app = express();
const port = process.env.PORT || 8080;

app.use(cors());
app.use(express.json({ limit: '100mb' }));
app.use(express.urlencoded({ extended: true, limit: '100mb' }));

app.use((req, res, next) => {
  console.log(`Incoming request: [${req.method}] ${req.url}`);
  console.log('Headers:', JSON.stringify(req.headers, null, 2));
  console.log('Body:', JSON.stringify(req.body, null, 2));
  next();
});

// Load OpenAPI spec for Swagger
const openapiPath = path.join(__dirname, '../openapi.yaml');
const openapiFile = fs.readFileSync(openapiPath, 'utf8');
const swaggerDocument = yaml.parse(openapiFile);

app.use('/docs', swaggerUi.serve, swaggerUi.setup(swaggerDocument));

const provider = new BlackboxProvider();

// OpenAI Compatible Chat Completions Endpoint
app.post(['/chat/completions', '/responses'], async (req, res) => {
  try {
    let authHeader = req.headers.authorization || '';
    
    // 1. Validate the incoming Proxy API Key (if configured or defaulting to xyz)
    const proxySecret = process.env.PROXY_API_KEY || 'xyz';
    if (proxySecret) {
      const incomingKey = authHeader.replace('Bearer ', '').trim();
      if (incomingKey !== proxySecret) {
        return res.status(401).json({ error: 'Unauthorized: Invalid PROXY_API_KEY' });
      }
      // Strip our proxy key so the Blackbox provider defaults to the free keys
      authHeader = '';
    }
    
    const apiKey = authHeader.replace('Bearer ', '').trim();
    
    let {
      model = 'gpt-4o-mini',
      temperature,
      max_tokens,
      top_p,
      stream = false,
      tools,
      tool_choice,
      parallel_tool_calls
    } = req.body;

    // Translate tools to standard OpenAI format
    if (tools && Array.isArray(tools)) {
      const validTools = [];
      for (const t of tools) {
        // If it's already perfectly formatted
        if (t.type === 'function' && t.function && t.function.name) {
          validTools.push(t);
          continue;
        }
        
        // Otherwise, extract the core properties and force the wrapper
        const inner = t.function || t[t.type] || t;
        const name = inner.name || t.name || t.type;
        
        if (name) {
          validTools.push({
            type: 'function',
            function: {
              name: name,
              description: inner.description || t.description || '',
              parameters: inner.parameters || t.parameters || { type: "object", properties: {} }
            }
          });
        }
      }
      tools = validTools.length > 0 ? validTools : undefined;
    }

    if (tool_choice && typeof tool_choice === 'object') {
      if (tool_choice.type && tool_choice.type !== 'function' && tool_choice.type !== 'custom') {
        tool_choice = 'auto';
      }
    }

    let messages = req.body.messages;

    // Translation layer for Codex 'responses' wire API
    if (!messages && req.body.input && Array.isArray(req.body.input)) {
      messages = [];
      if (req.body.instructions) {
        messages.push({ role: 'system', content: req.body.instructions });
      }
      for (const item of req.body.input) {
        if (item.type === 'message') {
          let role = item.role;
          if (role === 'developer') role = 'system';
          
          let textContent = '';
          if (Array.isArray(item.content)) {
            for (const block of item.content) {
              if (block.type === 'input_text' || block.type === 'text') {
                textContent += block.text || '';
              }
            }
          } else if (typeof item.content === 'string') {
            textContent = item.content;
          }
          messages.push({ role, content: textContent });
        }
      }
      req.body.messages = messages;
    }

    if (!messages || !Array.isArray(messages)) {
      return res.status(400).json({ 
        error: 'messages array is required',
        received_body: req.body 
      });
    }

    const options: CompletionOptions = {
      temperature,
      max_tokens,
      top_p,
      tools,
      tool_choice,
      parallel_tool_calls,
    };

    console.log("FINAL TOOLS ARRAY SENT TO BLACKBOX:", JSON.stringify(tools, null, 2));
    console.log("FINAL TOOL CHOICE:", JSON.stringify(tool_choice, null, 2));

    if (stream) {
      const generator = provider.streamChatCompletion(apiKey, messages, model, options);

      // Pull the first chunk before sending SSE headers so auth/model failures
      // can still return the correct HTTP status instead of a 200 SSE error.
      const first = await generator.next();

      res.setHeader('Content-Type', 'text/event-stream');
      res.setHeader('Cache-Control', 'no-cache');
      res.setHeader('Connection', 'keep-alive');
      
      try {
        if (!first.done) {
          res.write(`data: ${JSON.stringify(first.value)}\n\n`);
        }
        for await (const chunk of generator) {
          res.write(`data: ${JSON.stringify(chunk)}\n\n`);
        }
        res.write('data: [DONE]\n\n');
        res.end();
      } catch (err: any) {
        // Stream may have already started, so we just log and end
        console.error('Streaming error:', err);
        res.write(`data: ${JSON.stringify({ error: err.message || 'Stream failed' })}\n\n`);
        res.end();
      }
    } else {
      const response = await provider.chatCompletion(apiKey, messages, model, options);
      res.json(response);
    }
  } catch (err: any) {
    const status = err.status || 500;
    res.status(status).json({
      error: {
        message: err.message || 'Internal Server Error',
        status
      }
    });
  }
});

// List Models Endpoint
app.get('/models', (req, res) => {
  const models = [
    'gpt-5.5',
    'gpt-5.4',
    'gpt-5.4-mini',
    'gpt-4o-mini',
    'custom/blackbox-base-2',
    'minimax-m2',
    'minimax-m2.7',
    'moonshotai/kimi-k2.6',
    'custom/blackbox-pro',
    'deepseek-v3',
    'deepseek-r1',
    'gemini-2.5-pro',
    'z-ai/glm-4.7',
    'claude-3-5-sonnet-20241022',
    'claude-3-7-sonnet-20250219',
    'o1',
    'o3-mini'
  ];

  res.json({
    object: 'list',
    data: models.map((model) => ({
      id: model,
      object: 'model',
      created: 1700000000,
      owned_by: 'blackbox'
    }))
  });
});

// Anthropic Compatible Messages Endpoint
app.post('/v1/messages', async (req, res) => {
  try {
    let authHeaderRaw = req.headers['x-api-key'] || req.headers.authorization || '';
    let authHeader = Array.isArray(authHeaderRaw) ? authHeaderRaw[0] : authHeaderRaw;
    
    const proxySecret = process.env.PROXY_API_KEY || 'xyz';
    if (proxySecret) {
      let incomingKey = authHeader;
      if (incomingKey.startsWith('Bearer ')) incomingKey = incomingKey.replace('Bearer ', '');
      incomingKey = incomingKey.trim();
      
      if (incomingKey !== proxySecret) {
        return res.status(401).json({ type: 'error', error: { type: 'authentication_error', message: 'invalid x-api-key' }});
      }
      authHeader = '';
    }
    
    const {
      model,
      messages,
      system,
      max_tokens,
      temperature,
      stream = false
    } = req.body;

    if (stream) {
      return res.status(400).json({ error: { message: 'Streaming is not yet supported for Anthropic endpoints.' } });
    }

    // Convert Anthropic messages to OpenAI format
    const openAIMessages: any[] = [];
    if (system) {
      openAIMessages.push({ role: 'system', content: system });
    }
    for (const msg of messages) {
      // Simplistic conversion for now
      let content = '';
      if (typeof msg.content === 'string') {
        content = msg.content;
      } else if (Array.isArray(msg.content)) {
        content = msg.content.map((c: any) => c.text || '').join('');
      }
      openAIMessages.push({ role: msg.role as any, content });
    }

    const options: CompletionOptions = {
      temperature,
      max_tokens,
    };

    const response = await provider.chatCompletion(authHeader, openAIMessages as any, model, options);

    // Convert OpenAI response back to Anthropic format
    const contentText = response.choices[0]?.message?.content || '';
    res.json({
      id: response.id || `msg_${Date.now()}`,
      type: 'message',
      role: 'assistant',
      model: response.model || model,
      content: [{ type: 'text', text: contentText }],
      stop_reason: 'end_turn',
      usage: {
        input_tokens: response.usage?.prompt_tokens || 0,
        output_tokens: response.usage?.completion_tokens || 0
      }
    });

  } catch (err: any) {
    const status = err.status || 500;
    res.status(status).json({
      type: 'error',
      error: {
        type: 'api_error',
        message: err.message || 'Internal Server Error'
      }
    });
  }
});

app.listen(port, () => {
  console.log(`🚀 Blackbox Provider running at http://localhost:${port}`);
  console.log(`📚 Swagger documentation available at http://localhost:${port}/docs`);
});
