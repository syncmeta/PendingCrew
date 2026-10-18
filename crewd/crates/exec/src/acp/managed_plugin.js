// Production managed-task bridge. No synthetic model, history claim, or global gate.
// The calling process owns the PTY; Core alone supplies durable operation IDs.
/** @param {import("claude-code").On} on */
export function register(on) {
  let running = false;
  let stopped = false;
  let sequence = 0;
  let channel = Promise.resolve();
  /** @type {string | null | undefined} */ let socket = null;
  /** @type {string | null | undefined} */ let token = null;
  /** @type {string | null | undefined} */ let generation = null;
  /** @type {string | null} */ let sessionId = null;
  /** @type {string | null} */ let activeTurn = null;
  /** @param {import("claude-code").EngineInterface} $ @param {string} method @param {unknown} params */
  async function rpc($, method, params) {
    if (stopped || typeof socket !== 'string' || typeof token !== 'string' || typeof generation !== 'string') {
      throw new Error('managed Core channel unavailable');
    }
    const serial = channel;
    /** @type {() => void} */ let release = () => {};
    channel = new Promise(resolve => { release = resolve; });
    await serial;
    try {
      if (stopped || sequence >= Number.MAX_SAFE_INTEGER) throw new Error('managed sequence closed');
      const body = JSON.stringify({generation, token, sessionId, sequence: ++sequence, method, params});
      if (new TextEncoder().encode(body).length > 7 * 1024 * 1024) throw new Error('managed frame exceeds bounds');
      const response = await $.http.fetch('http://127.0.0.1:0/', {method:'POST', socketPath:socket, body});
      if (!response.ok || response.status !== 200) throw new Error('managed Core refused operation');
      const reply = JSON.parse(response.text);
      if (reply.generation !== generation || reply.sequence !== sequence) throw new Error('managed ACK binding changed');
      return reply.result;
    } catch (error) {
      stopped = true; // Never reconnect or re-fetch a possibly released operation.
      throw error;
    } finally { release(); }
  }
  /** @param {import("claude-code").EngineInterface} $ */
  async function pump($) {
    while (!stopped) {
      const operation = await rpc($, 'poll', {});
      if (operation === null) { await $.clock.sleep(100); continue; }
      if (typeof operation !== 'object' || typeof operation.id !== 'string') throw new Error('managed operation identity refused');
      // The poll reply is a single release of an already-durable Core intent.
      // No retry, including when submit or the result ACK is unknown.
      try {
        let value;
        if (operation.kind === 'prompt' && typeof operation.text === 'string') {
          value = await $.prompt.submit({text:operation.text, asUser:true});
        } else if (operation.kind === 'cancel' && typeof operation.turnId === 'string' && operation.turnId === activeTurn) {
          await $.turn.abort({turnId:operation.turnId});
          value = {cancelAcknowledged:true, turnId:operation.turnId};
        } else { throw new Error('managed operation target refused'); }
        await rpc($, 'returned', {operationId:operation.id, value});
      } catch (_) {
        try { await rpc($, 'unknown', {operationId:operation.id}); } catch (_) {}
        stopped = true;
      }
    }
  }
  on('session.start', async ($, event, next) => {
    const result = await next(event);
    if (running) return result;
    running = true;
    socket = await $.env.get('PCW_MANAGED_SOCKET');
    token = await $.env.get('PCW_MANAGED_TOKEN');
    generation = await $.env.get('PCW_MANAGED_GENERATION');
    sessionId = await $.session.id();
    await rpc($, 'hello', {version:await $.session.version()});
    $.clock.after(0, () => { void pump($).catch(() => { stopped = true; }); });
    return result;
  });
  // Native events are session observations. A matching-looking text or the next
  // turn is never converted into completion of a Core operation.
  on('turn.start', async ($, event, next) => {
    activeTurn = event.turnId;
    try { await rpc($, 'turnStarted', event); } catch (_) {}
    return next(event);
  });
  on('turn.complete', async ($, event, next) => {
    if (!event.agentId && activeTurn === event.turnId) activeTurn = null;
    try { await rpc($, 'turnCompleted', event); } catch (_) {}
    return next(event);
  });
  // Each invocation holds its actual native continuation. Core creates a unique
  // callback identity; no tool-name/input hash is used to find a continuation.
  on('classic.PermissionRequest', async ($, event, next) => {
    try {
      const pending = await rpc($, 'permission', event);
      if (typeof pending?.callbackId !== 'string') throw new Error('permission identity missing');
      while (!stopped) {
        const answer = await rpc($, 'permissionAnswer', {callbackId:pending.callbackId});
        if (answer === null) { await $.clock.sleep(100); continue; }
        if (answer.behavior !== 'allow' && answer.behavior !== 'deny') throw new Error('permission answer refused');
        // Only this invocation returns this one decision. No permanent rules,
        // updatedInput, session-wide grant, or keyboard fallback is generated.
        return {decision:answer};
      }
    } catch (_) {}
    return {decision:{behavior:'deny',message:'PendingCrew permission connection unavailable'}};
  });
}
