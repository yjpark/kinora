# Kinora

**An agent-native knowledge system where ideas move, connect, and compose.**

Kinora is a knowledge management system built for the age of AI agents. Named after the [Kinora](https://en.wikipedia.org/wiki/Kinora) — an early motion picture device that turned individual cards into coherent moving images — Kinora turns discrete fragments of knowledge into composable, queryable, living structures.

## Beads setup

```bash
bd dolt remote add origin git+https://github.com/edger-dev/kinora.git

```
## Why Kinora?

Traditional note-taking tools assume a human is typing and organizing. But increasingly, AI agents are the primary authors — researching, summarizing, drafting, linking. If your workflow has shifted from *writing notes* to *reviewing and composing agent-generated knowledge*, you need a system designed for that reality.

Kinora replaces the outdated file-and-folder paradigm with a graph of versioned, typed, composable knowledge fragments — **kinos** — that agents create and humans curate.

## Core Vocabulary

| Term | Meaning |
|------|---------|
| **Kino** | The atomic unit of knowledge. A small, self-contained fragment — one idea, one claim, one observation. Not a bullet point, not a document. A semantic unit meant to be composed with others. |
| **Kinograph** | The knowledge graph. The full collection of kinos and their typed relationships. |
| **Mosaic** | A composition of kinos assembled into a larger structure — a blog post, a report, a design doc. Kinos are not copied into mosaics; they are referenced and rendered. |

## Principles

### 1. Markdown as Persistence, Database as Working Memory

Kino files on disk are the source of truth — durable, portable, human-readable in emergencies but not meant for hand-editing. Think of markdown as the hard disk: it's for persistence. The database layer (SurrealDB) is working memory: it's where agents and humans actually interact with the data. We work with memory and sync back to disk, not the other way around.

### 2. The Kino is the Atomic Unit

A kino captures one idea. It's larger than a bullet point but smaller than a document — a self-contained fragment of knowledge with clear boundaries. Each kino is a single markdown file with structured frontmatter carrying its identity, provenance, version chain, tags, and typed links.

```yaml
---
id: 01JD5KX7...
created: 2026-03-06T10:00:00Z
author: claude/opus
supersedes: null
tags: [architecture, microvm]
links:
  - target: 01JD5KW3...
    type: elaborates
  - target: 01JD5KY9...
    type: contradicts
status: draft
---

Firecracker uses a minimalist device model with only 4 emulated devices,
which is what enables its <125ms boot time. This is a deliberate tradeoff
against hardware compatibility.
```

### 3. Append-Only Versioning

Edits never mutate a kino. Instead, they create a new kino that supersedes the previous version. All versions are first-class citizens on disk — nothing is buried in git history. v1 and v5 sit side by side as equals. The resolution of "what's current?" happens entirely in the database layer: the latest version is simply the one that nothing else supersedes.

This gives you full history for free, eliminates merge conflicts, and makes every past state of knowledge directly accessible.

### 4. Agent-First Authorship

The system is designed for AI agents as primary authors. This means:

- **Provenance is mandatory.** Every kino records who created it, when, and from what context.
- **Writes are conflict-free.** Agents create new files; they never need to coordinate on editing shared files.
- **No organizational knowledge required.** Agents don't need to understand where things "go." They create kinos with appropriate tags and links; structure emerges from the graph.

### 5. Typed, Bidirectional Links

Relationships between kinos are explicit and semantic — not just "these are linked" but *how* they relate. Link types include `elaborates`, `contradicts`, `supersedes`, `supports`, `depends-on`, and others. These are stored as graph edges in SurrealDB, enabling rich traversal queries like "show me everything that contradicts this claim within 2 hops."

### 6. Composition Over Organization

There are no folders. Long-form content is never written monolithically — it is assembled from kinos via queries and ordering. A mosaic defines a composition: "take these kinos, arrange them in this order, render with these transitions." The kinos themselves never move or get filed. The same kino can appear in multiple mosaics, potentially in different variations.

The system has three core operations:
- **Append** — create a kino
- **Link** — relate kinos to each other
- **Compose** — assemble kinos into a mosaic (a view or document)

### 7. Emergent Structure

Organization arises from links, tags, and queries — not from prescribed hierarchies. An agent can cluster related kinos. A human can pin a useful query as a saved view. The kinograph feels more like a database with dynamic views than a filesystem with folders.

### 8. Human Review as First-Class Workflow

Agents produce; humans curate. Kinos have lifecycle states — `draft`, `reviewed`, `accepted`, `archived`. The system supports a review queue: "show me everything agents wrote since yesterday" is a first-class operation, not something you cobble together by scanning file modification dates.

### 9. Schema Evolution Through Linting and Migration

The kino format will evolve. Since humans don't hand-edit markdown files, automated tooling can rewrite all kinos to conform to new schemas — adding fields, renaming relationship types, restructuring frontmatter. The linter is not just a validator; it's a migration tool. Frontmatter fields are kept in a canonical sorted order so diffs are stable and meaningful.

## Architecture Overview

```
┌─────────────────────────────────────────────┐
│              Agents & Humans                │
│         (create, link, compose, review)      │
└──────────────────┬──────────────────────────┘
                   │
┌──────────────────▼──────────────────────────┐
│           SurrealDB (Working Memory)         │
│                                              │
│  • Document store for kino content           │
│  • Graph edges for typed links               │
│  • Query engine for views and composition    │
│  • Version chain resolution                  │
│  • WASM build for browser-based access       │
└──────────────────┬──────────────────────────┘
                   │ sync
┌──────────────────▼──────────────────────────┐
│        Markdown Files (Persistence)          │
│                                              │
│  • One file per kino version                 │
│  • Frontmatter carries all metadata          │
│  • Append-only: new versions, never mutate   │
│  • Lintable, migratable, git-friendly        │
└─────────────────────────────────────────────┘
```

## Technology Choices

| Layer | Choice | Rationale |
|-------|--------|-----------|
| Persistence | Markdown + YAML frontmatter | Portable, durable, human-readable, tool-agnostic |
| Working Memory | SurrealDB | Document + graph hybrid, WASM build for browser, elegant query language |
| IDs | ULID or similar | Sortable by time, globally unique, no coordination needed |
| Schema Enforcement | Custom linter / migrator | Keeps all kinos conformant as the format evolves |

## Status

Kinora is in the early design phase. This document captures the foundational principles from initial brainstorming. The following areas are identified for deeper exploration:

- **Append-only versioning semantics** — version chain resolution, garbage collection, branching
- **The three core operations** — precise semantics of append, link, and compose
- **Mosaic composition model** — how kinos are selected, ordered, and rendered into long-form output
- **Agent integration patterns** — how agents discover, read, and write kinos in practice
- **SurrealDB schema design** — tables, edges, indexes, and the sync protocol with markdown
- **Browser-based kinograph explorer** — SurrealDB WASM for offline-capable browsing UI

