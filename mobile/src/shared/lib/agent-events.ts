import type { AgentEvent, Message } from "../client/types";

/** Rust owns message identity, role and initial content. Deltas are idempotent against newer snapshots. */
export function applyAgentOutput(messages: Message[], event: AgentEvent): Message[] {
  return applyAgentOutputs(messages, [event]);
}

/** Streamed output almost always targets the last message: search from the end. */
function indexFromEnd(messages: readonly Message[], id: string): number {
  for (let index = messages.length - 1; index >= 0; index--) if (messages[index].id === id) return index;
  return -1;
}

/**
 * One frame's events for one transcript, copying the list at most once. `gap` hears a delta
 * that starts past the text the transcript holds (loaded mid-stream, so text is missing).
 */
export function applyAgentOutputs(messages: Message[], events: readonly AgentEvent[], gap?: () => void): Message[] {
  let out = messages;
  const own = () => { if (out === messages) out = [...messages]; return out; };
  for (const event of events) {
    const index = indexFromEnd(out, event.messageId);
    if (event.message) {
      if (index < 0) { own().push(event.message); continue; }
      const current = out[index];
      if (current.content.length >= event.message.content.length) continue;
      own()[index] = { ...event.message, activity: current.activity ?? event.message.activity, streaming: current.streaming };
      continue;
    }
    if (index < 0 || out[index].content.length < event.offset) gap?.();
    if (index < 0 || out[index].content.length !== event.offset) continue;
    own()[index] = { ...out[index], content: out[index].content + event.chunk };
  }
  return out;
}
