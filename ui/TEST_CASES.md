# Codex RC UI Test Cases

## Common UI Principles

1. Turn continuity
   Every event for the same `turnId` must stay inside one response block from prompt to completion.
2. Readable answer continuity
   Final answer text must remain readable while work logs stay progressive and secondary.
3. Stable local control
   Sending, replying, stopping, and approving must operate on the intended local turn without being hijacked by unrelated streamed events.

## Automated Run

```bash
npm install
npm run test:ui
```

## TC-01 Single Turn Continuity

- Goal: One prompt never splits across multiple blocks.
- Steps:
  1. Start the app and send one prompt that triggers reasoning, commands, and file changes.
  2. Watch the turn until completion.
- Expected:
  1. Only one turn card is created for that prompt.
  2. Reasoning, command output, approvals, and file changes stay in that card's work log.
  3. Final completion status updates the same card.

## TC-02 Answer Segment Spacing

- Goal: Consecutive assistant message segments remain readable.
- Steps:
  1. Send a prompt likely to produce multiple assistant message chunks around tool usage.
  2. Observe the main answer area while streaming.
- Expected:
  1. Text within a single streamed segment stays continuous.
  2. When a new assistant message item continues the answer, it is separated from the prior item with a newline break when needed.
  3. Words do not collapse together across item boundaries.

## TC-03 Approval Routing

- Goal: Approval prompts attach to the correct turn.
- Steps:
  1. Send a prompt that requests command execution approval.
  2. Before answering the approval, confirm the related turn card and work log.
- Expected:
  1. The approval section appears inside the matching turn card only.
  2. Approve or decline affects that same turn.
  3. No unrelated turn card receives the approval UI.

## TC-04 Reply Continuity

- Goal: Follow-up prompts remain on the intended thread.
- Steps:
  1. Complete one turn.
  2. Send a follow-up reply.
- Expected:
  1. The follow-up creates a new turn card in the same session thread.
  2. The thread badge remains stable.
  3. Stop targets only the active local turn.

## TC-05 Disconnect Recovery

- Goal: Active turns fail visibly and consistently on disconnect.
- Steps:
  1. Start a long-running turn.
  2. Stop the server or interrupt the socket connection.
- Expected:
  1. Any in-progress turn card changes to `Disconnected`.
  2. The inline error explains that the connection was lost before completion.
  3. The composer returns to a non-working state after reconnect logic starts.
