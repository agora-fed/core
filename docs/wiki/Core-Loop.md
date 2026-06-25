# The core loop, explained

```
1. propose      citizen files a proposal directed at a mandate/campaign        (proposals)
2. cluster      consensus embeds it (pgvector), merges near-duplicates         (consensus)
3. vote         citizens support; tallies update                              (votes)
4. threshold    support crosses the directed threshold                         (proposals)
5. notify       SLA clock starts; the official is pushed on mobile             (consequence + notify)
6. respond      official answers within SLA  ── OR ── SLA expires (silence)    (consequence)
7. record       outcome written to the permanent public scorecard             (scorecard)
```

Each step is a separate crate, connected **only** by events (see the `core` event catalog). No crate
calls another crate's functions directly. This is what makes the loop auditable and the codebase
parallelizable.
