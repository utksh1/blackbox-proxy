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
app.use(express.json());

// Load OpenAPI spec for Swagger
const openapiPath = path.join(__dirname, '../openapi.yaml');
const openapiFile = fs.readFileSync(openapiPath, 'utf8');
const swaggerDocument = yaml.parse(openapiFile);

app.use('/docs', swaggerUi.serve, swaggerUi.setup(swaggerDocument));

const provider = new BlackboxProvider();

// OpenAI Compatible Chat Completions Endpoint
app.post('/chat/completions', async (req, res) => {
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
    
    const {
      model = 'gpt-4o-mini',
      messages,
      temperature,
      max_tokens,
      top_p,
      stream = false,
      tools,
      tool_choice,
      parallel_tool_calls
    } = req.body;

    if (!messages || !Array.isArray(messages)) {
      return res.status(400).json({ error: 'messages array is required' });
    }

    const options: CompletionOptions = {
      temperature,
      max_tokens,
      top_p,
      tools,
      tool_choice,
      parallel_tool_calls,
    };

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

app.listen(port, () => {
  console.log(`🚀 Blackbox Provider running at http://localhost:${port}`);
  console.log(`📚 Swagger documentation available at http://localhost:${port}/docs`);
});
