# LC-815/LC-816: the sidecar transcription agent (LC-810 stage 3/4). A LiveKit
# Agents worker; runs on bun (the toolchain the rest of the repo already uses).

# Production dependencies only.
FROM oven/bun:1 AS deps
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --production --frozen-lockfile

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
COPY package.json bun.lock tsconfig.json ./
RUN bun install --frozen-lockfile
COPY src ./src
COPY test ./test
RUN bun x tsc --noEmit

# Runtime. This is the LAST stage, so a plain `docker build` (and the compose
# overlay) produces the runtime image WITHOUT running the test stage.
FROM oven/bun:1 AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends gosu \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1001 appuser \
    && useradd --uid 1001 --gid 1001 --no-create-home appuser
WORKDIR /app
ENV NODE_ENV=production \
    APP_USER=appuser \
    DATA_DIRS=""
COPY --from=deps --chown=appuser:appuser /app/node_modules ./node_modules
COPY --chown=appuser:appuser package.json tsconfig.json ./
COPY --chown=appuser:appuser src ./src
COPY --chmod=0755 gosu-entrypoint.sh /usr/local/bin/gosu-entrypoint.sh

# Starts as root; gosu-entrypoint.sh drops to appuser via gosu (DATA_DIRS is
# empty, the agent writes nothing at runtime). A compose `user:` pin skips the
# remap and runs directly as the pinned uid.
ENTRYPOINT ["/usr/local/bin/gosu-entrypoint.sh"]
# `start` is the LiveKit Agents production worker subcommand. The worker connects
# to LIVEKIT_URL, registers under LETS_CHAT_TRANSCRIBE_AGENT_NAME, and waits for
# lets-chat to dispatch it into a room (LC-814).
CMD ["bun", "run", "src/agent.ts", "start"]
