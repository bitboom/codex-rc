import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { JSDOM } from 'jsdom';

const html = await readFile(new URL('../ui/index.html', import.meta.url), 'utf8');

function flush(ms = 0) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function createMockWebSocketClass() {
  const sockets = [];

  return class MockWebSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSING = 2;
    static CLOSED = 3;
    static instances = sockets;

    constructor(url) {
      this.url = url;
      this.readyState = MockWebSocket.CONNECTING;
      this.sent = [];
      this.onopen = null;
      this.onmessage = null;
      this.onclose = null;
      this.onerror = null;
      sockets.push(this);
    }

    send(data) {
      this.sent.push(JSON.parse(data));
    }

    open() {
      this.readyState = MockWebSocket.OPEN;
      this.onopen?.();
    }

    receive(payload) {
      this.onmessage?.({ data: JSON.stringify(payload) });
    }

    close() {
      if (this.readyState === MockWebSocket.CLOSED) {
        return;
      }
      this.readyState = MockWebSocket.CLOSED;
      this.onclose?.();
    }
  };
}

async function bootApp() {
  const MockWebSocket = createMockWebSocketClass();

  const dom = new JSDOM(html, {
    pretendToBeVisual: true,
    runScripts: 'dangerously',
    url: 'http://localhost:9876/',
    beforeParse(window) {
      const realSetTimeout = globalThis.setTimeout.bind(globalThis);

      window.WebSocket = MockWebSocket;
      window.requestAnimationFrame = (cb) => realSetTimeout(() => cb(Date.now()), 0);
      window.cancelAnimationFrame = (id) => clearTimeout(id);
      window.setTimeout = (fn, delay = 0, ...args) => {
        if (delay >= 1500) {
          return 1;
        }
        return realSetTimeout(fn, delay, ...args);
      };
      window.clearTimeout = clearTimeout;
    },
  });

  await flush();

  const socket = MockWebSocket.instances.at(-1);
  assert(socket, 'UI did not open a WebSocket connection');
  socket.open();
  socket.receive({ type: 'ready' });
  await flush();

  const { window } = dom;
  const { document } = window;

  return {
    dom,
    window,
    document,
    sockets: MockWebSocket.instances,
    currentSocket() {
      return MockWebSocket.instances.at(-1);
    },
    async receive(payload, socket = this.currentSocket()) {
      socket.receive(payload);
      await flush();
    },
    async close(socket = this.currentSocket()) {
      socket.close();
      await flush();
    },
    setPrompt(text) {
      const prompt = document.getElementById('prompt');
      prompt.value = text;
      prompt.dispatchEvent(new window.Event('input', { bubbles: true }));
    },
    clickSend() {
      document.getElementById('send').click();
    },
    getTurnCards() {
      return [...document.querySelectorAll('.turn-card')];
    },
    getLatestTurnCard() {
      return this.getTurnCards().at(-1) ?? null;
    },
    getAnswerText(card) {
      return card?.querySelector('.turn-answer')?.textContent ?? '';
    },
    getLogSections(card) {
      return [...(card?.querySelectorAll('.log-section') ?? [])];
    },
    getApprovalSections(card) {
      return this.getLogSections(card).filter((section) => section.classList.contains('approval'));
    },
    cleanup() {
      dom.window.close();
    },
  };
}

function userMessageItem(id, text) {
  return {
    id,
    type: 'userMessage',
    content: [{ type: 'text', text, text_elements: [] }],
  };
}

function reasoningItem(id) {
  return {
    id,
    type: 'reasoning',
    summary: [],
    content: [],
  };
}

function commandItem(id, command) {
  return {
    id,
    type: 'commandExecution',
    command,
    status: 'inProgress',
  };
}

function fileChangeItem(id, changes = []) {
  return {
    id,
    type: 'fileChange',
    changes,
    status: 'completed',
  };
}

function agentMessageItem(id, text = '') {
  return {
    id,
    type: 'agentMessage',
    text,
    phase: 'final_answer',
  };
}

async function testSingleTurnContinuity() {
  const app = await bootApp();

  try {
    app.setPrompt('Inspect the repo and summarize the UI.');
    app.clickSend();

    const outbound = app.currentSocket().sent.at(-1);
    assert.equal(outbound.type, 'start_turn');
    assert.equal(app.getTurnCards().length, 1);

    await app.receive({ type: 'turn_started', thread_id: 'thread-1', turn_id: 'turn-1' });
    await app.receive({
      type: 'item_started',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'user-1',
      item_type: 'userMessage',
      item: userMessageItem('user-1', 'Inspect the repo and summarize the UI.'),
    });
    await app.receive({
      type: 'item_started',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'reason-1',
      item_type: 'reasoning',
      item: reasoningItem('reason-1'),
    });
    await app.receive({
      type: 'delta',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'reason-1',
      delta_type: 'reasoning',
      content: 'Checking routing and streaming behavior.',
    });
    await app.receive({
      type: 'item_completed',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'reason-1',
      item_type: 'reasoning',
      item: { ...reasoningItem('reason-1'), summary: 'Routing' },
    });
    await app.receive({
      type: 'item_started',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'cmd-1',
      item_type: 'commandExecution',
      item: commandItem('cmd-1', 'rg --files'),
    });
    await app.receive({
      type: 'approval_request',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'cmd-1',
      request_id: 'approval-1',
      method: 'item/commandExecution/requestApproval',
      detail: { command: 'rg --files', cwd: '/workspace', reason: 'Inspect files' },
    });
    await app.receive({
      type: 'delta',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'cmd-1',
      delta_type: 'commandOutput',
      content: 'ui/index.html\nsrc/main.rs\n',
    });
    await app.receive({
      type: 'item_completed',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'cmd-1',
      item_type: 'commandExecution',
      item: { ...commandItem('cmd-1', 'rg --files'), status: 'completed', exitCode: 0, durationMs: 320 },
    });
    await app.receive({
      type: 'item_started',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'patch-1',
      item_type: 'fileChange',
      item: fileChangeItem('patch-1'),
    });
    await app.receive({
      type: 'item_completed',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'patch-1',
      item_type: 'fileChange',
      item: fileChangeItem('patch-1', [
        { kind: 'update', path: 'ui/index.html', diff: '@@ mock diff @@' },
      ]),
    });
    await app.receive({
      type: 'item_started',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'agent-1',
      item_type: 'agentMessage',
      item: agentMessageItem('agent-1'),
    });
    await app.receive({
      type: 'delta',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'agent-1',
      delta_type: 'agentMessage',
      content: 'Summary is ready.',
    });
    await app.receive({
      type: 'item_completed',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      item_id: 'agent-1',
      item_type: 'agentMessage',
      item: agentMessageItem('agent-1', 'Summary is ready.'),
    });
    await app.receive({
      type: 'turn_completed',
      thread_id: 'thread-1',
      turn_id: 'turn-1',
      status: 'completed',
      error: null,
    });

    const cards = app.getTurnCards();
    assert.equal(cards.length, 1, 'Expected one turn card for one prompt');
    assert.equal(cards[0].dataset.state, 'completed');
    assert.equal(app.getApprovalSections(cards[0]).length, 1, 'Expected approval inside the same turn card');
    assert.equal(app.getLogSections(cards[0]).length, 4, 'Expected all work-log sections to stay in the same card');
  } finally {
    app.cleanup();
  }
}

async function testAnswerSegmentSpacing() {
  const app = await bootApp();

  try {
    app.setPrompt('Write a two-part answer.');
    app.clickSend();

    await app.receive({ type: 'turn_started', thread_id: 'thread-2', turn_id: 'turn-2' });
    await app.receive({
      type: 'item_started',
      thread_id: 'thread-2',
      turn_id: 'turn-2',
      item_id: 'agent-a',
      item_type: 'agentMessage',
      item: agentMessageItem('agent-a'),
    });
    await app.receive({
      type: 'delta',
      thread_id: 'thread-2',
      turn_id: 'turn-2',
      item_id: 'agent-a',
      delta_type: 'agentMessage',
      content: 'Alpha',
    });
    await app.receive({
      type: 'item_completed',
      thread_id: 'thread-2',
      turn_id: 'turn-2',
      item_id: 'agent-a',
      item_type: 'agentMessage',
      item: agentMessageItem('agent-a', 'Alpha'),
    });
    await app.receive({
      type: 'item_started',
      thread_id: 'thread-2',
      turn_id: 'turn-2',
      item_id: 'agent-b',
      item_type: 'agentMessage',
      item: agentMessageItem('agent-b'),
    });
    await app.receive({
      type: 'delta',
      thread_id: 'thread-2',
      turn_id: 'turn-2',
      item_id: 'agent-b',
      delta_type: 'agentMessage',
      content: 'Beta',
    });

    const answer = app.getAnswerText(app.getLatestTurnCard());
    assert.match(answer, /^Alpha\n\nBeta$/, 'Expected a paragraph break between separate answer items');
  } finally {
    app.cleanup();
  }
}

async function testApprovalRouting() {
  const app = await bootApp();

  try {
    app.setPrompt('First prompt');
    app.clickSend();
    await app.receive({ type: 'turn_started', thread_id: 'thread-3', turn_id: 'turn-3a' });
    await app.receive({
      type: 'turn_completed',
      thread_id: 'thread-3',
      turn_id: 'turn-3a',
      status: 'completed',
      error: null,
    });

    app.setPrompt('Second prompt');
    app.clickSend();
    const replyMessage = app.currentSocket().sent.at(-1);
    assert.equal(replyMessage.type, 'reply');
    assert.equal(replyMessage.thread_id, 'thread-3');

    await app.receive({ type: 'turn_started', thread_id: 'thread-3', turn_id: 'turn-3b' });

    await app.receive({
      type: 'approval_request',
      thread_id: 'thread-3',
      turn_id: 'turn-3a',
      item_id: 'cmd-old',
      request_id: 'approval-old',
      method: 'item/commandExecution/requestApproval',
      detail: { command: 'git status', cwd: '/workspace' },
    });

    const [firstCard, secondCard] = app.getTurnCards();
    assert.equal(app.getApprovalSections(firstCard).length, 1, 'Delayed approval should stay with the original turn');
    assert.equal(app.getApprovalSections(secondCard).length, 0, 'Delayed approval must not leak into the active turn');
  } finally {
    app.cleanup();
  }
}

async function testReplyContinuity() {
  const app = await bootApp();

  try {
    app.setPrompt('Start a thread');
    app.clickSend();
    await app.receive({ type: 'turn_started', thread_id: 'thread-4', turn_id: 'turn-4a' });
    await app.receive({
      type: 'turn_completed',
      thread_id: 'thread-4',
      turn_id: 'turn-4a',
      status: 'completed',
      error: null,
    });

    app.setPrompt('Follow up');
    app.clickSend();

    const outbound = app.currentSocket().sent.at(-1);
    assert.equal(outbound.type, 'reply');
    assert.equal(outbound.thread_id, 'thread-4');
    assert.equal(outbound.prompt, 'Follow up');

    await app.receive({ type: 'turn_started', thread_id: 'thread-4', turn_id: 'turn-4b' });

    assert.equal(app.getTurnCards().length, 2, 'Reply should create a second turn card in the same thread');
    assert.match(app.document.getElementById('threadId').textContent, /^thread-4/, 'Thread badge should stay on the same thread');
  } finally {
    app.cleanup();
  }
}

async function testDisconnectRecovery() {
  const app = await bootApp();

  try {
    app.setPrompt('Run something long');
    app.clickSend();
    await app.receive({ type: 'turn_started', thread_id: 'thread-5', turn_id: 'turn-5' });

    await app.close();

    const card = app.getLatestTurnCard();
    assert.equal(card.dataset.state, 'interrupted');
    assert.match(
      card.querySelector('.turn-inline-error')?.textContent ?? '',
      /Connection was lost before the turn finished/,
    );
  } finally {
    app.cleanup();
  }
}

const tests = [
  ['TC-01 single turn continuity', testSingleTurnContinuity],
  ['TC-02 answer segment spacing', testAnswerSegmentSpacing],
  ['TC-03 approval routing', testApprovalRouting],
  ['TC-04 reply continuity', testReplyContinuity],
  ['TC-05 disconnect recovery', testDisconnectRecovery],
];

let failures = 0;

for (const [name, testFn] of tests) {
  try {
    await testFn();
    console.log(`PASS ${name}`);
  } catch (error) {
    failures += 1;
    console.error(`FAIL ${name}`);
    console.error(error);
  }
}

if (failures > 0) {
  process.exitCode = 1;
} else {
  console.log(`PASS all ${tests.length} UI test cases`);
}
