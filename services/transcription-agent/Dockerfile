# LC-815: the sidecar transcription agent (LC-810 stage 3). A LiveKit Agents
# worker; runs on bun (the toolchain the rest of the repo already uses).
FROM oven/bun:1 AS deps
WORKDIR /app
COPY package.json ./
# No lockfile is committed for this leaf service; resolve at build time.
RUN bun install --production

FROM oven/bun:1
WORKDIR /app
ENV NODE_ENV=production
COPY --from=deps /app/node_modules ./node_modules
COPY package.json tsconfig.json ./
COPY src ./src
# `start` is the LiveKit Agents production worker subcommand. The worker connects
# to LIVEKIT_URL, registers under LETS_CHAT_TRANSCRIBE_AGENT_NAME, and waits for
# lets-chat to dispatch it into a room (LC-814).
CMD ["bun", "run", "src/agent.ts", "start"]
