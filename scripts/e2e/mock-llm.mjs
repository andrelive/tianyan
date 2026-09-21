//! Zero-dependency OpenAI-compatible mock LLM server for Tianyan E2E tests.
//!
//! Implements the subset of the OpenAI wire protocol that the backend's
//! async_openai client speaks, with fully deterministic responses so real
//! frontend-backend integration tests (Playwright) can assert exact values:
//!
//!   POST /v1/chat/completions  non-stream: fixed JSON completion
//!   POST /v1/chat/completions  stream:     SSE chunks, LF line endings only
//!   POST /v1/embeddings                     fixed normalized 768-dim vector
//!   GET  /v1/models                         e2e-chat + e2e-embed
//!
//! Usage:
//!   node scripts/e2e/mock-llm.mjs              # port from MOCK_LLM_PORT (default 8765)
//!
//! Exports (so smoke scripts can import the same constants without duplicating):
//!   REPLY  — the canned assistant reply, split across 3 stream chunks.
//!   EMBED_DIM — default embedding vector length.
//!   ASK_TRIGGER / ASK_QUESTION — clarify-scenario constants (see below).
//!
//! Scenario switching:
//!   When the LAST user message contains ASK_TRIGGER, the mock returns a single
//!   `ask_user` tool_call (finish_reason=tool_calls) instead of plain text. The
//!   backend intercepts ask_user in the main loop → NeedsClarification → the
//!   GUI shows the clarification bubble. A subsequent clarify round (whose user
//!   message is the answer text, no trigger) falls back to the normal REPLY.
//!   All other requests stay deterministic single-round text (never emit tool_calls).
//!
//! Notes:
//!   - Binds 127.0.0.1 only.
//!   - Accepts any Authorization header (the backend sends "Bearer e2e-key").
//!   - SSE uses LF only (never CRLF) and flushes after every line.
//!   - Request/connection errors are swallowed so the server never crashes.

import http from 'node:http';
import { pathToFileURL } from 'node:url';

export const REPLY = '你好，我是天演 E2E 模拟助手，这条回复来自 mock-llm。';
export const EMBED_DIM = 768;
export const ASK_TRIGGER = '触发追问';
export const ASK_QUESTION = 'E2E 追问测试：你更喜欢哪个颜色？';
const STREAM_CHUNKS = 3;
const MAX_BODY_BYTES = 1 * 1024 * 1024;

const HOST = '127.0.0.1';
const PORT = Number(process.env.MOCK_LLM_PORT ?? 8765);

const created = () => Math.floor(Date.now() / 1000);

/** True when the LAST message is a user message containing ASK_TRIGGER
 * (clarify scenario). Only the most recent user turn counts: a clarify round
 * carries the whole history (whose user messages may contain the trigger),
 * but its last message is the tool result — it must fall back to REPLY. */
function wantsAskUser(messages) {
  if (!Array.isArray(messages) || messages.length === 0) return false;
  const last = messages[messages.length - 1];
  return (
    last &&
    last.role === 'user' &&
    typeof last.content === 'string' &&
    last.content.includes(ASK_TRIGGER)
  );
}

/** Split text into exactly `parts` contiguous pieces (roughly equal length). */
function splitText(text, parts) {
  const size = Math.ceil(text.length / parts);
  const out = [];
  for (let i = 0; i < parts; i += 1) {
    out.push(text.slice(i * size, (i + 1) * size));
  }
  return out;
}

/** Fixed, normalized constant vector: [1, 0, 0, ..., 0] of the given length. */
function makeEmbedding(dim) {
  const vector = new Array(dim).fill(0);
  vector[0] = 1;
  return vector;
}

function sendJson(res, status, payload) {
  const body = JSON.stringify(payload);
  res.writeHead(status, {
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(body),
  });
  res.end(body);
}

function sendError(res, status, message) {
  sendJson(res, status, {
    error: { message, type: 'invalid_request_error' },
  });
}

/** Read the request body, returning null when malformed or oversized. */
function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    let size = 0;
    req.on('data', (chunk) => {
      size += chunk.length;
      if (size > MAX_BODY_BYTES) {
        resolve(null);
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => resolve(Buffer.concat(chunks)));
    req.on('error', () => resolve(null));
  });
}

function handleChatCompletions(req, res, body) {
  let payload;
  try {
    payload = JSON.parse(body);
  } catch {
    sendError(res, 400, 'invalid JSON body');
    return;
  }
  const model = typeof payload.model === 'string' ? payload.model : 'e2e-chat';
  const stream = payload.stream === true;
  // Clarify scenario: last user message contains ASK_TRIGGER.
  const ask = wantsAskUser(payload.messages);

  if (!stream) {
    const completion = {
      id: 'mock-1',
      object: 'chat.completion',
      created: created(),
      model,
      choices: [
        {
          index: 0,
          message: ask
            ? {
                role: 'assistant',
                content: null,
                tool_calls: [
                  {
                    id: 'call_ask_1',
                    type: 'function',
                    function: {
                      name: 'ask_user',
                      arguments: JSON.stringify({
                        questions: [
                          {
                            question: ASK_QUESTION,
                            options: [
                              { label: '蓝色', description: '冷静的色调' },
                              { label: '绿色', description: '自然的色调' },
                            ],
                          },
                        ],
                      }),
                    },
                  },
                ],
              }
            : { role: 'assistant', content: REPLY },
          finish_reason: ask ? 'tool_calls' : 'stop',
        },
      ],
      usage: { prompt_tokens: 5, completion_tokens: 9, total_tokens: 14 },
    };
    sendJson(res, 200, completion);
    return;
  }

  // SSE streaming: LF line endings ONLY, flushed after every line.
  const chunk = (delta) => ({
    id: 'mock-1',
    object: 'chat.completion.chunk',
    created: created(),
    model,
    choices: [{ index: 0, delta, finish_reason: null }],
  });

  res.writeHead(200, {
    'Content-Type': 'text/event-stream',
    'Cache-Control': 'no-cache',
    Connection: 'keep-alive',
  });

  // Clarify scenario: single tool_call chunk + finish_reason=tool_calls.
  if (ask) {
    res.write(
      `data: ${JSON.stringify(chunk({
        tool_calls: [
          {
            index: 0,
            id: 'call_ask_1',
            type: 'function',
            function: {
              name: 'ask_user',
              arguments: JSON.stringify({
                questions: [
                  {
                    question: ASK_QUESTION,
                    options: [
                      { label: '蓝色', description: '冷静的色调' },
                      { label: '绿色', description: '自然的色调' },
                    ],
                  },
                ],
              }),
            },
          },
        ],
      }))}\n\n`,
    );
    res.write(
      `data: ${JSON.stringify({
        id: 'mock-1',
        object: 'chat.completion.chunk',
        created: created(),
        model,
        choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }],
      })}\n\n`,
    );
    res.write('data: [DONE]\n\n');
    res.end();
    return;
  }

  // 3 content chunks (delta carries role + content part).
  for (const part of splitText(REPLY, STREAM_CHUNKS)) {
    res.write(`data: ${JSON.stringify(chunk({ role: 'assistant', content: part }))}\n\n`);
  }
  // Final chunk: empty delta + finish_reason "stop".
  res.write(
    `data: ${JSON.stringify({
      id: 'mock-1',
      object: 'chat.completion.chunk',
      created: created(),
      model,
      choices: [{ index: 0, delta: {}, finish_reason: 'stop' }],
    })}\n\n`,
  );
  res.write('data: [DONE]\n\n');
  res.end();
}

function handleEmbeddings(req, res, body) {
  let payload;
  try {
    payload = JSON.parse(body);
  } catch {
    sendError(res, 400, 'invalid JSON body');
    return;
  }
  const model = typeof payload.model === 'string' ? payload.model : 'e2e-embed';
  const dim =
    Number.isInteger(payload.dimensions) && payload.dimensions > 0
      ? payload.dimensions
      : EMBED_DIM;
  // Echo one embedding per input item so batch calls map 1:1 (fixed constant
  // vector → cosine similarity is always 1.0 → deterministic retrieval hits).
  const input = payload.input;
  const count = Array.isArray(input) ? input.length : 1;
  const data = Array.from({ length: count }, (_, i) => ({
    index: i,
    object: 'embedding',
    embedding: makeEmbedding(dim),
  }));
  sendJson(res, 200, {
    object: 'list',
    data,
    model,
    usage: { prompt_tokens: 4, total_tokens: 4 },
  });
}

function handleModels(req, res) {
  sendJson(res, 200, {
    object: 'list',
    data: [
      { id: 'e2e-chat', object: 'model' },
      { id: 'e2e-embed', object: 'model' },
    ],
  });
}

const server = http.createServer((req, res) => {
  // Never crash on client disconnects / aborted requests.
  req.on('error', () => {});
  res.on('error', () => {});

  const { pathname } = new URL(req.url ?? '/', `http://${req.headers.host ?? '127.0.0.1'}`);

  const finish = (status) => {
    console.log(`[mock-llm] ${req.method} ${pathname} -> ${status}`);
  };

  if (req.method === 'POST' && pathname === '/v1/chat/completions') {
    readBody(req).then((body) => {
      if (body === null) {
        sendError(res, 400, 'invalid request body');
        finish(400);
        return;
      }
      handleChatCompletions(req, res, body.toString('utf8'));
      finish(200);
    });
    return;
  }

  if (req.method === 'POST' && pathname === '/v1/embeddings') {
    readBody(req).then((body) => {
      if (body === null) {
        sendError(res, 400, 'invalid request body');
        finish(400);
        return;
      }
      handleEmbeddings(req, res, body.toString('utf8'));
      finish(200);
    });
    return;
  }

  if (req.method === 'GET' && pathname === '/v1/models') {
    handleModels(req, res);
    finish(200);
    return;
  }

  sendError(res, 404, 'not found');
  finish(404);
});

// Malformed HTTP (bad request line / headers): drop the socket, keep serving.
server.on('clientError', (_err, socket) => {
  socket.destroy();
});

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) {
  server.listen(PORT, HOST, () => {
    console.log(`[mock-llm] listening on http://${HOST}:${PORT}`);
  });
  server.on('error', (err) => {
    console.error(`[mock-llm] failed to listen: ${err.message}`);
    process.exitCode = 1;
  });

  const shutdown = () => {
    console.log('[mock-llm] shutting down');
    server.close(() => process.exit(0));
    // Force-exit if a keep-alive connection lingers.
    setTimeout(() => process.exit(0), 500).unref();
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
}

export { server };
