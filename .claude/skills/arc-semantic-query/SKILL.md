---
name: arc-semantic-query
description: >
  Protocol for retrieving semantically similar historical changes and rationale. 
  Use when implementing search, embedding lookups, or `arc query --vibe`.
---

# arc-semantic-query

## Purpose
`arc query --vibe` allows users and AI agents to search the Spacetime DAG not just by text, but by intent and semantic structure.

## Query Protocol

1. **Feature Extraction:** Do not just search raw strings. Convert the user's query into structured intent features (e.g., "Find all times a Trait was extracted from a Struct").
2. **Graph & Embedding Search:** 
   - Search the Intent Graph metadata in `redb`.
   - If using semantic embeddings (e.g., local vector store), hash the AST structural shape to find mathematically similar past refactors.
3. **Rank and Explain:** 
   - Return ranked matches.
   - Crucially, the code must generate a concise "why-this-matched" rationale based on the AST overlap or causal relationship.
4. **Uncertainty Annotations:**
   - If the intent graph lacks context or the embedding distance is weak, include explicit uncertainty annotations in the result. Never hallucinate absolute confidence for a fuzzy match.

## Anti-Patterns
- Never use simple `grep` or regex over the repository history to fulfill a semantic query.
- Do not trigger expensive LLM API calls for deterministic graph searches; rely on the local vector/intent indexes first.