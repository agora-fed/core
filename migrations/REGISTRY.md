# Migration number registry

`dsoc-db` embeds ONE migrator (`sqlx::migrate!("../../migrations")`), so all migrations share this
directory and must have unique, sequential numeric prefixes. To let many crate-owner agents add
migrations in parallel without collisions, each crate owns a **10-wide range**. Files are named
`NNNN_<slug>_<description>.sql`. Enforced by `scripts/check-migration-numbers.sh` in CI.

| Range | Crate | Range | Crate |
|------:|-------|------:|-------|
| 0001  | baseline (db/core) | 0300 | proposals |
| 0100  | auth      | 0310 | votes |
| 0110  | notify    | 0320 | comments |
| 0120  | events    | 0330 | debates |
| 0130  | consensus | 0340 | meetings |
| 0140  | moderation| 0350 | budgets |
| 0150  | admin     | 0360 | surveys |
| 0200  | mandates  | 0370 | accountability |
| 0210  | processes | 0380 | consequence |
| 0220  | assemblies| 0390 | scorecard |
| 0230  | initiatives | 0400 | federation (likely none) |
| 0240  | consultations | | |

Rule: a migration may `REFERENCES` another crate's table ONLY for the core identity tables
(`org`, `citizen`, `mandate`). Enforced by `scripts/check-fk-targets.sh`.
