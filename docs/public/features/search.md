# Search

Two kinds of search in one box: lexical by default, semantic when the operator wires it.

## Lexical search

Every message you can read is indexed for full-text search as it posts. The search box at the top of the sidebar queries that index.

Supported syntax:
- `word` matches the word.
- `"two words"` matches the exact phrase.
- `from:@user` scopes to one author.
- `in:#room` scopes to one room.
- `has:file`, `has:image`, `has:link` filters by attachment kind.

Results are grouped by room with the matching message highlighted. Clicking a result jumps to the message in context.

## Semantic search

If the operator has set `LETS_CHAT_EMBEDDINGS_URL`, the server stores an embedding for every message and the search box adds a semantic path. A query like `the escalation runbook from last Thursday` returns messages whose meaning matches, not only their words.

Semantic results are mixed with lexical results and ranked together. Semantic search runs only on messages you can read; it does not leak content from rooms you are not in.

## Find related

Right-clicking a message gives "Find related". That query is a semantic search seeded by the message's embedding. Useful for picking up a thread that is not strictly a reply, like earlier discussions of the same incident.

## What is not indexed

- File contents. Lets chat does not extract text from uploaded PDFs or documents; only file names and link previews are searchable.
- Deleted messages. A tombstone remains in the room but the body is gone.
- Direct messages you are not a party to. DMs are private to their participants.

## Index maintenance

The index updates in-line as messages post. There is no background reindex to run.

If you turn on an embeddings endpoint on an existing deployment, run the one-shot backfill at `/admin/search/backfill`. It walks every message, computes a vector, and stores it. The backfill is idempotent and resumable.
