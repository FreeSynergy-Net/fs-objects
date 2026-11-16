# Changelog

Version = tag.

## 0.0.6

- Pin: the optional dependency `fs-types` (feature `v5`) now comes directly from its own repository `FreeSynergy/fs-types`, tag v0.0.1 (ADR 0093), no longer through `fs-interfaces` v0.0.26. The lock has exactly one `fs-types` source and no `fs-interfaces` entry any more. The workflow uses `fs-release` v0.0.10, and `.fs-lock-sources.toml` makes the lock check strict. No code, table or API change; steps 1 to 7 and their fingerprints are unchanged.
- Tests: `standard_sqlite_reads_the_store_file` writes under the test target (`CARGO_TARGET_TMPDIR`), not `/tmp` (RAM), and removes its folder also when it fails; a new test runs the standard-SQLite check (`python3 -I`, with the `-wal` file) on a file migrated from the v0.0.4 shape; the `ON CONFLICT` of step 5 (`WORDS_OLD`) is tested like that of step 6.
- Auf Deutsch: Pin: `fs-types` (Feature `v5`) kommt jetzt direkt aus dem eigenen Repo `FreeSynergy/fs-types` v0.0.1 (ADR 0093), nicht mehr ueber `fs-interfaces` v0.0.26; im Lock steht genau eine `fs-types`-Quelle und kein `fs-interfaces`. Der Workflow nutzt `fs-release` v0.0.10, die Lock-Pruefung ist per `.fs-lock-sources.toml` streng. Kein Code, keine Tabelle, keine API geaendert; Schritte 1 bis 7 und ihre Fingerabdruecke bleiben. Tests: der SQLite-Test schreibt unter dem Test-Target statt in `/tmp` (RAM) und raeumt auch bei Fehlschlag auf; neuer Test prueft eine aus der v0.0.4-Form migrierte Datei mit Standard-SQLite (mit `-wal`); das `ON CONFLICT` von Schritt 5 ist wie das von Schritt 6 getestet.

## 0.0.5

- The typed log (v5) knows the five AS2 activity kinds `Add`, `Remove`, `Join`, `Leave` and `Move` (`v5::log::Kind`, 13 kinds now), which fs-tasks needs. Per ADR 0086 (addendum 2026-11-16) the kinds are a word table `fs_word` (`kind`, `code`, `builtin`, ...), not a `CHECK` list: new steps 4 to 7 of the store's own domain create it, insert the eight old kinds and the five new ones as built-in words (`ON CONFLICT (kind, code) DO UPDATE SET builtin = 1`), and rebuild `activity` (staging tables, no `ALTER TABLE ... RENAME`) so that `(kind_set, kind)` is a foreign key into it; old rows are copied unchanged. Steps 1 to 3 and their fingerprints are unchanged (a test pins them). New column `activity.kind_set` (always `activity_kind`). The word table is the single table `fs_word` (ADR 0086 open point 6), because turso 0.8.3 renames the autoindex of any table whose name starts with `activity_` when `activity` is renamed, which standard SQLite then refuses. No pin or dependency changes.
- Auf Deutsch: Das typisierte Protokoll (v5) kennt zusaetzlich die fuenf AS2-Aktivitaetsarten `Add`, `Remove`, `Join`, `Leave`, `Move` (jetzt 13 Arten). Nach ADR 0086 (Nachtrag 16.11.) stehen die Arten in einer Wort-Tabelle `fs_word` statt in einer `CHECK`-Liste: neue Schritte 4 bis 7 der eigenen Domain legen sie an, tragen die acht alten und die fuenf neuen Arten als eingebaute Woerter ein und bauen `activity` so um, dass `(kind_set, kind)` per Fremdschluessel darauf zeigt; alte Zeilen bleiben erhalten. Schritte 1 bis 3 und ihre Fingerabdruecke bleiben unveraendert. Pins und Abhaengigkeiten unveraendert.

## 0.0.4

- Only the pin changes: the optional dependency `fs-types` (feature `v5`) points to `fs-interfaces` tag v0.0.26 (was v0.0.22), so a program that uses `fs-objects` with `v5` together with `fs-rpc` has exactly one `fs-types` in its lock (ADR 0092 draft). No code, table or API change.
- Auf Deutsch: Nur der Pin aendert sich: `fs-types` (Feature `v5`) kommt jetzt aus `fs-interfaces` v0.0.26 (vorher v0.0.22). Kein Code, keine Tabelle, keine API geaendert.
