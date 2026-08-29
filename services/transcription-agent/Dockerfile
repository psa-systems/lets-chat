# LC-815/LC-816: the sidecar transcription agent (LC-810 stage 3/4). A LiveKit
# Agents worker; runs on bun (the toolchain the rest of the repo already uses).

# Production dependencies only.
FROM oven/bun:1 AS deps
WORKDIR /app
COPY package.json ./
# No lockfile is committed for this leaf service; resolve at build time.
RUN bun install --production

# LC-816: unit tests, run in CI via `docker buildx build --target test` (the CI
# runner has no bun; work happens inside docker, like the desktop build). The
# tests import only local modules - not the LiveKit SDK - so no install is
# needed here, and a failing `bun test` fails the image build.
FROM oven/bun:1 AS test
WORKDIR /app
COPY package.json tsconfig.json ./
COPY src ./src
COPY test ./test
RUN bun test

# LC-841: compile the LiveKit wiring against the pinned SDK. The test stage
# above imports only the pure modules, never the SDK, which is how agent.ts
# shipped with two type errors (a type-only import and a WorkerOptions field
# that does not exist) that no check could see. This stage does the full
# install (SDK + typescript) and typechecks src/ and test/; CI runs it via
# `docker buildx build --target typecheck`. Not on the runtime path.
FROM oven/bun:1 AS typecheck
WORKDIR /app
COPY package.json tsconfig.json ./
RUN bun install
COPY src ./src
COPY test ./test
RUN bun x tsc --noEmit

# Runtime. This is the LAST stage, so a plain `docker build` (and the compose
# overlay) produces the runtime image WITHOUT running the test stage.
FROM oven/bun:1 AS runtime
WORKDIR /app
ENV NODE_ENV=production
COPY --from=deps /app/node_modules ./node_modules
COPY package.json tsconfig.json ./
COPY src ./src
# `start` is the LiveKit Agents production worker subcommand. The worker connects
# to LIVEKIT_URL, registers under LETS_CHAT_TRANSCRIBE_AGENT_NAME, and waits for
# lets-chat to dispatch it into a room (LC-814).
CMD ["bun", "run", "src/agent.ts", "start"]
