// LC-815: the agent joins under a reserved identity prefix so lets-chat's
// huddle_sfu.js filters it out of the roster (it must never render a tile).
// Kept in lockstep with livekit::AGENT_IDENTITY_PREFIX on the server (LC-814).

export const AGENT_IDENTITY_PREFIX = 'agent-';

/** The reserved LiveKit identity the agent joins as, for a given agent name. */
export function agentIdentity(agentName: string): string {
  return `${AGENT_IDENTITY_PREFIX}${agentName}`;
}

/** Whether an identity belongs to a dispatched agent (mirrors the server filter). */
export function isAgentIdentity(identity: string): boolean {
  return identity.startsWith(AGENT_IDENTITY_PREFIX);
}
