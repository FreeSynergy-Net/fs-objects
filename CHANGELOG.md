# Changelog

Version = tag.

## 0.0.4

- Only the pin changes: the optional dependency `fs-types` (feature `v5`) points to `fs-interfaces` tag v0.0.26 (was v0.0.22), so a program that uses `fs-objects` with `v5` together with `fs-rpc` has exactly one `fs-types` in its lock (ADR 0092 draft). No code, table or API change.
- Auf Deutsch: Nur der Pin aendert sich: `fs-types` (Feature `v5`) kommt jetzt aus `fs-interfaces` v0.0.26 (vorher v0.0.22). Kein Code, keine Tabelle, keine API geaendert.
