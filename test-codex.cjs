const fs = require('fs');

async function run() {
  const payload = {
    "model": "gpt-5.5",
    "instructions": "You are Codex...",
    "input": [
      {
        "type": "message",
        "role": "user",
        "content": [{ "type": "input_text", "text": "Test" }]
      }
    ],
    "tools": [
      {
        "type": "tool_search",
        "tool_search": { "name": "tool_search", "description": "search" }
      },
      {
        "type": "function",
        "name": "multi_tool_use.parallel",
        "description": "parallel"
      }
    ],
    "tool_choice": "auto",
    "stream": true
  };

  const res = await fetch('http://localhost:8080/responses', {
    method: 'POST',
    headers: { 
      'Content-Type': 'application/json',
      'Authorization': 'Bearer xyz'
    },
    body: JSON.stringify(payload)
  });

  console.log('Status:', res.status);
  
  const text = await res.text();
  console.log('Response body:', text);
}

run().catch(console.error);
