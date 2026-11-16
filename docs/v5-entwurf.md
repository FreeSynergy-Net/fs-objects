# fs-objects v5: draft (typed tables)

Status: **draft for review**, 2026-11-16. No migration code, no schema change in `crates/fs-objects/src`. This file is the
requirement list and the design sketch that the first user (fs-calendar) and the second user (fs-tasks) put on the next
schema version. Opus reviews it before anything is built (pilot `programm-grundgeruest`, plan step O3 (b)).

Marks: **[ADR n]** decided · **[probe]** the embedded engine was tested (`crates/fs-objects/tests/v5_probe.rs`, turso 0.8.3) ·
**[sketch]** this draft proposes · **[open]** the reviewer or the strategy decides · **[not checked]** not read or not tried.

Inputs: ADR 0086 (typed storage, accepted 2026-11-15, incl. the two additions: cipher exception, money), ADR 0085 (accepted,
closed 2026-11-16), ADR 0020/0030/0042/0073/0029/0067; `DESIGN.md` of this repo (schema v4); fs-calendar plan `PLAN.md`
section 2 and the **locked** trait folder `appointments_v1/` (folder hash SHA-256 `2b7fc98b5bf777a0c4f32ddf6d9c2b3ac5c784425df0ef2fe61c31c8ec5519b8`, lock file `.scratch/sperren/appointments_v1.sha256`, re-read on 2026-11-16 after the earlier candidate `f8e7fd1f...`); fs-tasks `model/task.rs`.

## 1. What changes, in one page

| v4 (today, `turso/schema.rs`) | v5 |
|---|---|
| `objects.content` = domain standard as JSON text | gone. The domain registers typed tables; one row of the envelope plus rows of the domain tables |
| `objects.original` BLOB, `original_type` | table `object_original` (the one BLOB exception, ADR 0086 item 4) |
| `objects.addressing`, `objects.sensitive` JSON | tables `object_addressing`, `object_protection` |
| `activities.payload` JSON (`ChangeSet{before, after}`) | tables `activity` + `activity_change` (field, old value, new value, typed) |
| `index_fields(object_id, key, value TEXT)` | gone; typed queries over typed columns |
| revision of a row = hash of the JSON text (in the programs) | column `objects.rev` (counter), compare-and-set in SQL |
| `start_time`/`end_time` in the envelope | gone from the envelope (D2); domain tables carry their own typed time columns |
| times as text or micro-seconds, dates as `TEXT` in `index_fields` | four distinct logical types, all stored as `INTEGER` (section 2) |

The unchanged parts: `meta`, `unlogged_ids`, `purged_objects`, store identity (`store_id`), encryption of the file, the `Store`
facade (extended, section 6), UUIDv7 ids, the activity log being append-only, undo/redo semantics (checked again in R10).

## 2. Column types

### 2.1 Decision O5: `INTEGER` for local time and date, for all programs

`LOCAL` (wall-clock time without zone) and `DATE` (day without time) are stored as integers in **every** program's tables, not as
ISO text. **[open, recommendation: yes]**

| Logical type | SQL | Encoding | Range / property |
|---|---|---|---|
| `INT` | `INTEGER` | i64 | [probe: full i64 stored, sorts as number] |
| `REAL` | `REAL` | f64 | coordinates, measurements; never money. **NaN and `+-inf` are refused by the layer** (R2, also for `Value::Real` in `activity_change`). **[probe]** the engine does not: NaN is stored as `NULL` (a nullable column silently loses the value, `CHECK (x = x)` does not see it, only `NOT NULL` refuses it) and infinity is stored as it is. So the check is code, in the registry's write path |
| `BOOL` | `INTEGER` + `CHECK (x IN (0,1))` | 0/1 | |
| `TEXT` | `TEXT` | UTF-8 | human text only, never JSON |
| `UTC` | `INTEGER` | milliseconds since 1970-01-01T00:00:00Z (the trait's `Utc(i64)`) | an instant; sortable |
| `LOCAL` | `INTEGER` | seconds since 1970-01-01T00:00:00 **read as if UTC** (wall clock, no zone) | not an instant; sorts in wall-clock order; no fraction, no leap second (JSCalendar `LocalDateTime` has none) |
| `DATE` | `INTEGER` | days since 1970-01-01 (proleptic Gregorian, negative allowed) | a day, never converted (ADR 0085) |
| `ZONE` | `TEXT` | IANA name, or `/...` custom zone of the same object | checked on write (R2) |
| `ENUM(a,b,..)` | `TEXT` + `CHECK (x IN ('a','b',..))` | the standard's wording (RFC 8984 names) | [probe: CHECK ... IN enforced] |
| `REF` | `TEXT` | UUIDv7 text of an object (36 chars) | foreign key to `objects(id)` where it is one |
| `IRI` | `TEXT` | absolute IRI as written (`fs_types::Reference`) | never rewritten (ADR 0085 section 3) |
| `MONEY` | `INTEGER` + `<x>_currency TEXT` + `<x>_scale INTEGER` | smallest unit [ADR 0086 addition] | never `REAL` |

Composite types expand into column groups with fixed suffixes (the registry knows them, R1):

| Composite | Columns | Rule |
|---|---|---|
| `PERSON` (`PersonRef`) | `<x>_id IRI NOT NULL`, `<x>_art ENUM(profile,person,group,organization) NOT NULL`, `<x>_abzug REF NULL` | `_abzug` points to `person_snapshot` and may be non-null only in fields the domain marks "issued" (ADR 0085 rule "issued = frozen") |
| `FILE` (`FileRef`) | `<x>_iri IRI`, `<x>_sha256 TEXT` (**64 lower-case hex characters**, never a `BLOB`: ADR 0086; `CHECK (length(x) = 64)`; the trait's `[u8; 32]` is encoded and decoded by `fs-types`), `<x>_media_type TEXT` | checksum is part of the type (ADR 0086 addition) |
| `WHEN` (appointment time, due time) | see 2.3 | one shape for calendar and tasks |
| `CIPHER` (second exception) | `<x>_alg TEXT`, `<x>_nonce BLOB`, `<x>_key_ref REF`, `<x>_bytes BLOB` | the structure typed, only the bytes are bytes (ADR 0086 addition); not needed by calendar or tasks |

Why integers for `LOCAL`/`DATE` (checkable claims):

1. Range queries and order are numeric. ISO text sorts correctly only while every field is zero-padded and the year has four
   digits; negative years and years above 9999 break it. Integers have no such case.
2. Window search needs arithmetic (`utc_min < :to AND utc_max > :from`, day + n). With text the engine cannot compute.
3. One decoding rule in `fs-types` for all programs, instead of each program parsing text.
4. Cost: dumps are unreadable. Mitigation: the registry knows the logical type, so the admin view and `fs-objects dump`
   render `LOCAL` as `2026-11-17T09:00:00` and `DATE` as `2026-11-17` (R2). **[sketch]**
5. Rejected alternative: ISO `TEXT`. Readable and standard-shaped, but fails 1 and 2.

The engine does not check the logical type; it checks only `INTEGER` against `TEXT`, and only in `STRICT` tables:

- **[probe]** a plain table accepts `'x'` in an `INTEGER` column; a `STRICT` table refuses it ("cannot store TEXT value in INTEGER
  column"). So **every v5 table is `STRICT`**, and `BLOB` appears only in the exceptions of 2.4.
- **[probe]** generated columns (`GENERATED ALWAYS AS`) need an experimental flag: derived columns are written by code (R11), not
  by the engine.

### 2.2 The type registry

Two tables describe the schema to the store itself, so the rules in R2 and the check in R14 need no hand-kept list:

```
fs_domain (prefix TEXT PK, version INTEGER, description TEXT, applied UTC)
fs_column (domain TEXT, tbl TEXT, col TEXT, logical ENUM, nullable BOOL, derived BOOL, PRIMARY KEY (domain, tbl, col))
```

The tables of fs-objects itself (section 3), `fs_domain`, `fs_column` and `fs_migration` (section 7) are registered in `fs_column`
under the reserved domain `fs_`/`object`, like any other; otherwise R14 would flag the store's own tables.

### 2.3 The composite `WHEN` (calendar `When`, task `due`/`start`)

Follows the closed trait (`When::{Timed, Floating, AllDay}`) and ADR 0085 section 2.

| Column | Type | Meaning |
|---|---|---|
| `<x>_kind` | ENUM(timed, floating, all_day) | |
| `<x>_local` | LOCAL NULL | start wall clock (timed, floating) |
| `<x>_date` | DATE NULL | first day (all_day) |
| `<x>_zone` | ZONE NULL | timed only |
| `<x>_end_zone` | ZONE NULL | timed only, end in another zone |
| `<x>_zone_assumed` | BOOL | timed only (set by an import that guessed the zone) |
| `<x>_len_days`, `<x>_len_seconds` | INT | nominal days + exact seconds (RFC 8984 `Duration`) |
| `<x>_utc_min`, `<x>_utc_max` | UTC, derived | hull for the window search (see below) |

```
CHECK (kind = 'timed'    AND local IS NOT NULL AND date IS NULL AND zone IS NOT NULL
    OR kind = 'floating' AND local IS NOT NULL AND date IS NULL AND zone IS NULL AND zone_assumed = 0 AND end_zone IS NULL
    OR kind = 'all_day'  AND local IS NULL AND date IS NOT NULL AND zone IS NULL AND end_zone IS NULL AND zone_assumed = 0
                         AND len_seconds = 0 AND len_days >= 1)
```

`zone_assumed` is a `BOOL` and carries its own `CHECK (zone_assumed IN (0,1))`; the trait's `AllDay.days >= 1` is the
`len_days >= 1` above. **[probe: all_day with `end_zone`, with `len_days = 0` and a BOOL of 2 are refused, in a `STRICT` table
with FK and partial index]**

Hull (derived, written by code on every write and by `reindex`, R11): `timed`: exact start and end instant. `floating` and
`all_day`: the local interval widened by the largest zone offsets (**start - 16 h, end + 16 h**), so a window query finds every
candidate in any viewer zone; the provider then places the row exactly in the viewer zone (`view_zone` of the trait's `window`).
Window query: `utc_min < :to AND utc_max > :from`.

Offset range **[checked, not by a Rust probe]**: a scan of all IANA zones of the system tz database (Python `zoneinfo`, quarterly
samples 1800-2100, 2026-11-16) finds today's range -12 h..+14 h but also historical local mean times beyond it: `Asia/Manila`
-15:56, `Pacific/Palau` -15:02, `Pacific/Guam`/`Saipan` -14:21, `Pacific/Kosrae` -13:08 and, with a **positive** offset (Russian-era local mean times of places that lie geographically west of Greenwich), `America/Metlakatla` +15:14,
`Juneau` +15:02, `Sitka` +14:59, `Yakutat` +14:41, `Anchorage` +14:00:24 (all before 1900). So -14 h/+12 h would miss rows;
the draft takes **+-16 h** as the known bound; the margin is thin: `Asia/Manila` is only about 4 minutes inside 16 h. Known limit: a later tz database may add a larger offset; `reindex` (R11) after
a tz update and the build-time test "every zone of the database lies inside +-16 h" (part of the build checklist, section 9) catch it.

### 2.4 The exceptions (and only these)

`object_original.bytes BLOB` (received data, ADR 0086 item 4) and the `CIPHER` composite. R14 checks it.

## 3. Tables owned by fs-objects (all programs)

| Table | Columns | Notes |
|---|---|---|
| `objects` | `id REF PK`, `iri IRI NULL`, `type TEXT` (AS2/domain type name, an open set, no CHECK), `name TEXT NOT NULL`, `name_sort TEXT NOT NULL` (derived, as v4), `attributed_to PERSON NULL`, `published UTC`, `updated UTC`, `context REF NULL`, `in_reply_to REF NULL`, `describes REF NULL`, `rev INT NOT NULL`, `deleted UTC NULL`, `purge_after UTC NULL` | Replaces v4 `objects`; no `content`, `original*`, `addressing`, `sensitive`, `start_time`, `end_time`. `deleted`/`purge_after` carry the trash (calendar `restore`) |
| `object_original` | `object REF PK`, `media_type TEXT`, `bytes BLOB NULL`, `file FILE NULL`, `sha256 TEXT`, `received UTC`, `keep_until UTC NULL` | exactly one of `bytes`/`file`; immutable after insert (ADR 0030); retention per kind (ADR 0086 item 4) |
| `object_origin` | `object REF PK`, `channel ENUM(file,caldav,google,outlook,itip,hub,subscription)`, `source IRI`, `media_type TEXT`, `foreign_uid TEXT`, `foreign_recurrence_id LOCAL NULL`, `foreign_etag TEXT NULL`, `prod_id TEXT NULL`, `imported UTC`, `importer TEXT`, `rev_at_import INT`, `original FILE NULL` (= `original_iri`, `original_sha256`, `original_media_type`, all three NULL or none: CHECK) | shape taken from the trait's `Origin` (locked folder, `OriginChannel` has seven members, `Subscription` = feed URL, `source` is the feed IRI); "unchanged since import" is **computed**: `objects.rev = rev_at_import` (no stored flag). `original` mirrors `Origin.original: Option<FileRef>`: it is the **copy of the pointer** to the kept original, not a second store. If the bytes sit in `object_original.bytes`, then `original_iri = urn:sha256:<hex>` (the same form the trait gives `source` for files), `original_sha256` = `object_original.sha256`; if they sit in `fs-files`, it equals `object_original.file`. `NULL` = no original kept **[sketch]** |
| `object_addressing` | `object REF`, `role ENUM(to,cc,bto,bcc,audience)`, `target PERSON`, PK all | replaces JSON `addressing` |
| `object_protection` | `object REF`, `category ENUM` (ADR 0029 vocabulary), `level ENUM`, `set_by PERSON`, `set_at UTC`, PK (object, category) | replaces JSON `sensitive` and the `freesynergy.net:sensitive` key |
| `person_snapshot` | `id REF PK`, `person_id IRI`, `person_art ENUM`, `captured UTC`, `source REF NULL`, `doc_kind ENUM`, `issued DATE`, `keep_until DATE`, plus the fields the feed list of that `doc_kind` allows (name, address ...) | the frozen excerpt (ADR 0085 section 3); frozen = never updated (R8) |
| `activity` | `id REF PK` (UUIDv7, strictly monotonic as v4), `kind ENUM` (the AS2 kinds of v4), `object REF NOT NULL REFERENCES objects(id) ON DELETE CASCADE` (the row always exists: in v5 delete is trash, see R4), `actor PERSON`, `at UTC`, `target REF NULL` (no FK), `synced BOOL`, `redacted BOOL`, `explained BOOL` | no payload column. `redacted` keeps its v4 meaning. `explained` = 1: the `activity_change` rows are the complete diff (or the kind has none by nature); 0: migrated from a v4 payload without a domain migrator, no change rows (D7). Behaviour on delete: R4 |
| `activity_change` | `activity REF NOT NULL REFERENCES activity(id) ON DELETE CASCADE`, `seq INT`, `field TEXT`, `row_key TEXT NULL`, `value_kind ENUM(int,real,text,bool,utc,local,date,zone,object,iri,person,attendee)`, then for `old_` and again for `new_`: `_int INTEGER`, `_real REAL`, `_text TEXT`, `_person_id IRI`, `_person_art ENUM(profile,person,group,organization)`, `_person_abzug REF`, `_who_kind ENUM(actor,resource,location)`, `_who_id IRI`, `_who_art ENUM(profile,person,group,organization)`, `_who_abzug REF` (all NULL), PK (activity, seq) | one row per changed field; no JSON, no blob (ADR 0086 item 2). Every variant of the trait's `Value` has exactly one column group, chosen by `value_kind`: `int`,`bool`,`utc`,`local`,`date` -> `_int` (as 2.1); `real` -> `_real`; `text`,`zone`,`object` (REF text),`iri` (`Reference`) -> `_text`; `person` (`PersonRef`) -> `_person_id/_art/_abzug`; `attendee` (`Attendee::Actor(PersonRef)`, `Resource(Reference)`, `Location(Reference)`) -> `_who_kind` + `_who_id`, plus `_who_art/_who_abzug` for `actor` only. Table `CHECK`: the columns of other kinds are NULL (and `_who_art` is NULL unless `_who_kind = 'actor'`). A field that was absent/cleared has all columns of that side NULL. The value columns carry **no FK** (`object`, `_abzug`): history must survive the purge of what it names |
| `unlogged_ids`, `purged_objects`, `schema_version` | as v4 | already typed |
| `meta` | `key TEXT PK`, `value TEXT` for the scalar keys (`store_id`, `activity_high`); the `retention` key is **JSON text today** (`turso/retention.rs` stores `serde_json::to_string(&Retention)`): v5 moves it to a typed table `retention(kind TEXT PK, keep_ms INT NULL, max_steps INT NULL, ...)` with the fields of `Retention` **[read from the code, fields not listed here]** | the finding is part of R14: the check must find it |

`activity_change` replaces the before/after images of v4. Undo today applies the inverse of an image; in v5 it applies the inverse
of the changed fields (R10). The new snapshot type of "who did it" is `PERSON`, as in the trait's `HistoryEntry.actor`.

## 4. Requirement list for fs-objects v5

"K" = fs-calendar (first user, plan `programm-grundgeruest`), "T" = fs-tasks (second user, ADR 0045: two users before a
contract is final). The check column says how a reviewer verifies the requirement.

| Nr | Requirement | K | T | Check |
|---|---|---|---|---|
| R1 | A program **registers a domain**: prefix, version, tables (columns with logical types, keys, `CHECK`, indexes), own migrations. fs-objects creates the tables and records them in `fs_domain`/`fs_column`. Prefix `object`, `activity`, `person`, `fs_` are reserved | `calendar_` | `tasks_` (or shared `jscal_`, D1) | test: register two domains, open twice (idempotent), register a clashing prefix (error) |
| R2 | **Column types with checks on write**: `UTC`, `LOCAL`, `DATE`, `ZONE`, `ENUM`, `REF`, `IRI`, `PERSON`, `FILE`, `WHEN`, `MONEY`; `ZONE` against the tz database or the object's own custom zone; `LOCAL`/`DATE` must be real calendar values (30 Feb refused). The registry renders them for dumps | K: all | T: `DATE` for all-day due, `LOCAL`, `ZONE`, `PERSON` for assignee | table-driven test per type: good value stored and read back equal, bad value refused with the field name |
| R3 | **One transaction** over the envelope row, the domain rows (many tables), `activity` and `activity_change` (today `put_and_log`) | A write touches up to ~15 tables | tasks: task + labels + relations + board order | test: failure in the last table leaves no row anywhere ([probe: rollback across tables works]) |
| R4 | **Foreign keys with cascade**: domain child rows reference their parent row; deleting an object removes its domain rows and its `object_*` rows. Every connection must run `PRAGMA foreign_keys = ON` (it is per connection and not stored in the file). **Delete is always trash in v5**: a delete is `UPDATE objects SET deleted, purge_after` plus a delete activity; `DELETE FROM objects` exists **only** in purge. This differs from v4 (`Repository::delete` removes the row and logs the before image); `undo` of a delete in v5 = `restore` (clear `deleted`). **Writes never use `DELETE` + `INSERT` or `INSERT OR REPLACE`/`REPLACE` on `objects`** (v4 rewrites a row that way): that would fire the cascade and wipe the history; only `UPDATE` or `INSERT ... ON CONFLICT DO UPDATE` (upsert) are allowed, and the same holds for every table that has FK children. **Trash vs purge**: trash is `UPDATE objects SET deleted/purge_after` (no row is deleted, FKs do nothing, activity stays, `restore` works); **purge** is `DELETE FROM objects`: `activity.object` and `activity_change.activity` are `ON DELETE CASCADE`, so the purge removes the object's activity rows and their changes in the same statement (ADR 0029, R10); the id goes into `purged_objects` in the same transaction. `activity.target`, `objects.context/in_reply_to/describes` and the value columns of `activity_change` are **plain `REF`s without FK**: naming a purged object must neither fail nor cascade. A series: the cascade `master -> exception rows` removes the domain rows of the exceptions but **not their envelope rows**; purge deletes every `objects` row of the series in the one transaction (R3) | exception rows, participants, alerts ... | relations, labels | [probe: **off by default**, an orphan row is accepted until the pragma is set; with it, orphans are refused and `ON DELETE CASCADE` works; on TEXT keys, self-referencing `master`, four levels deep; a second connection and a reopened file start with the pragma **off**]. Test: connection opened by the store refuses an orphan; purge removes activity and changes, trash does not; a write that replaced the row would fail the test "history survives 100 updates" |
| R5 | **`CHECK` and partial unique indexes** (or an equal check in code) | `calendar_appointment` CHECK of 2.3; `UNIQUE (uid) WHERE master IS NULL`; `UNIQUE (master, recurrence_id) WHERE master IS NOT NULL` | `UNIQUE (uid)` | [probe: both accepted and enforced, also on TEXT (UUID) keys and in a `STRICT` table with CHECK and FK]. Test: violating rows refused |
| R6 | **Typed queries** replace `index_fields`: range on `UTC`/`LOCAL`/`DATE`, equality on `ENUM`/`REF`/`PERSON`/`IRI`, text contains on declared `TEXT` columns, order by any column, **keyset paging** `(sort key, id)`. Query values are typed, no `value_norm` string | window by hull, "my invitations" by `person_id`, search title | due range, by assignee, by label, by progress | test: 1000 rows, page through, same order as a full sort; plan check: the window query uses the index (`EXPLAIN QUERY PLAN`) **[not checked]** |
| R7 | **`rev` per envelope**: counter, bumped by every write of the object (envelope or any of its domain rows); `put_checked(expected_rev)` = `UPDATE ... WHERE id = ? AND rev = ?`, rows affected 0 = `Changed{current}` | trait `update(expected: Revision)` | `edit_checked` | test: two writers, the second gets `Changed`; no write on a no-change |
| R8 | The general tables of section 3, including `object_origin` (replaces the plan's `unchanged_since_import`) and `person_snapshot` (**insert-only**, no update path) | origin of imports; snapshot only for bookings later | assignee only | tests: update of a snapshot refused; `object_original` immutable (existing v4 rule) |
| R9 | **Migration hook per domain**, v4 -> v5, resumable, with a report (section 7) | `calendar` migrator | `tasks` migrator | see section 7 |
| R10 | Log and undo on typed changes: `put_and_log` writes `activity_change` rows from the domain's diff; `undo`/`redo` apply old/new values (same rules as v4: conflict if the object moved on; **except** that deleting is trash, R4, so undo of a delete is `restore`, not re-insertion from a before image); `purge_object` removes the activity rows and their changes (ADR 0029) | history of a Termin (`HistoryEntry.changes`) | task history | port the v4 tests of `tests/history.rs`, `log.rs` to typed changes |
| R11 | **Derived columns** are declared (`derived = true` in the registry) and recomputed by code: on every write, and by `reindex(domain)` after a tz update (ADR 0085 "expired zone rules") | `utc_min`, `utc_max`, fire times of alerts | due hull | test: change the tz data in a test double, `reindex` fixes the rows; derived columns never accepted from the caller |
| R12 | **Change feed** reports `(domain, table, key, kind)` after commit, not only the envelope | grid reload | board reload | test: observer sees one event per write transaction |
| R13 | **Types come from `fs-types`** (`Utc`, `LocalDateTime`, `Date`, `Zone`, `Reference`, `PersonRef`, `FileRef`); fs-objects defines no second copy (ADR 0085 Folgen) | stand-in in the trait file until locked | | compile: fs-objects depends on `fs-types`; no own time type |
| R14 | **Storage check** (workflow step `speicher-check`): after opening a store, no column is `BLOB` except `object_original.bytes` and the `CIPHER` composite; every column is declared in `fs_column`; every table is `STRICT`; heuristic: no `TEXT` value of a test database parses as a JSON object or array | | | a function `fs_objects::check::typed(&Store) -> Vec<Finding>` and a test using the calendar sample |
| R15 | **Encryption** of the file as today (`is_encrypted`, ADR 0024) covers all tables, domain tables included | | | existing `encryption.rs` test on a domain table |
| R16 | **Open question of the engine** (not probed): `ALTER TABLE DROP COLUMN`/`RENAME`, `CREATE TRIGGER`, expression indexes. Domain migrations may assume only what the table in section 8 lists | | | section 8 |

### What the calendar asks of R1 to R16, beyond the common part

- Series as rows: an exception of a series is its own `calendar_appointment` row (`master` + `recurrence_id`), so R4 cascade
  and R5 partial unique index are required, and R3 must write a series with its rule rows and exception rows in one transaction.
- `WHEN` composite with hull (2.3, R11) and the window query of R6; viewer-zone placement of floating and all-day rows stays in
  the provider.
- Trash: `objects.deleted`, `purge_after` and a `restore` that clears them (R3, R7).
- `PERSON` on participants with **no name column**; a participant may also be a resource or a location (trait `Attendee`): its
  columns are `who_kind ENUM(actor, resource, location)`, `who_id IRI`, `who_art ENUM NULL` (only for `actor`).
- Typed history with `row_key` = participant key (R10, trait `FieldChange`).
- Import: `object_origin`, `object_original` (ICS bytes per `uid`), a `Loss` report outside the store.

### What tasks asks beyond the calendar (T, second user)

- **All-day due** (`WHEN` kind `all_day`, `DATE`): fs-tasks cannot store a plain date today (`due` is an `Option<String>`); with
  R2 `DATE` and `WHEN` it can. Same composite for `start` if tasks gain one.
- **Estimated duration** = `INT` seconds (`estimatedDuration` is a duration, not an interval); `percentComplete INT`,
  `progress ENUM(needs-action,in-process,completed,failed,cancelled)`, `progressUpdated UTC`.
- **Relations with a gap** (`relate_with_gap`: `finishtostart` etc.): `tasks_relation(task REF, other_uid TEXT, relation ENUM, gap_seconds INT NULL)`; cycle check reads typed rows (R6).
- **Claim**: `claim_by PERSON`, `claim_until UTC` on the task row (compare-and-set R7 decides who wins).
- **Assignee** as `PERSON` (ADR 0085 removes `name`+`email` in `assign`); kind of actor (`ActorType`: person, program, service) is `PERSON.art` plus the markers of ADR 0076 **[open: where the AI marker is stored, People]**.
- **Boards**: `tasks_board`, `tasks_board_column(wip_limit INT NULL)`, `tasks_board_card(position INT)`: order is a typed integer column, not a JSON map `order`.
- **Recurrence** of tasks (RFC 8984 `Task` has the same recurrence members as `Event`): the rule tables are the same shape.

## 5. D1 and the other decisions for the reviewer

| Nr | Question | Options | Recommendation |
|---|---|---|---|
| D1 | JSCalendar `Event` and `Task` share participants, alerts, locations, virtual locations, links, recurrence rules, keywords, localizations (the `calendar_object!` macro of fs-jscalendar already shares them). One table family or two? | (a) `calendar_*` and `tasks_*` each with a copy; (b) one family `jscal_*` keyed by `object REF`, used by both | **(b)**: one schema, one migrator, the fs-jscalendar mapping written once (ADR 0045: two users). The plan's `calendar_participant` etc. become `jscal_participant`; only `calendar_appointment`/`calendar_collection` stay calendar-owned |
| D2 | Drop `start_time`/`end_time` from the envelope? | keep (but they are derived from the domain, two truths) / drop | drop: the hull of each domain is a typed column of its own table; the programs that set the envelope times today (fs-calendar) change with their migration |
| D3 | `REF` as 36-char `TEXT` or 16-byte `BLOB` | text / blob | text (ADR 0086 forbids blobs for own data; readable) |
| D4 | Type checks: engine only (`STRICT`) or also in code | STRICT + code checks per logical type (R2) | both: STRICT catches wrong storage class, R2 the meaning (`ZONE`, 30 Feb) |
| D5 | Migrate in place or into a new file | in place / new file next to the old | new file (section 7), old file untouched until the person confirms |
| D6 | `rev` counter per object or per row | per object | per object: the trait compares one `Revision` of the appointment |
| D7 | Is the activity `payload` of v4 migrated to typed changes or kept read-only in the old file | convert / keep | convert what the domain can explain; objects of unknown types keep their activity rows **without** changes, marked by the own column `activity.explained = 0` (`redacted` keeps its v4 meaning and is copied unchanged), and a note in the report |

## 6. Store API sketch (names are placeholders)

```rust
// Registration (Strategy: the domain describes itself)
pub trait Domain: Send + Sync {
    fn prefix(&self) -> &'static str;
    fn version(&self) -> u32;
    fn tables(&self) -> Vec<TableSpec>;            // columns with logical types, keys, checks, indexes
    fn migrations(&self) -> Vec<DomainStep>;       // v(n) -> v(n+1), same transaction as the registry row
    fn migrator(&self) -> Option<&dyn V4Migrator>; // R9
}
pub trait DomainStore {                            // part of the Store facade
    async fn register(&self, d: &dyn Domain) -> Result<()>;
    async fn write(&self, w: DomainWrite) -> Result<Rev>;      // R3: envelope + rows + activity, one transaction
    async fn read(&self, q: TypedQuery) -> Result<Rows>;       // R6
    async fn put_checked(&self, id: ObjectId, expected: Rev, w: DomainWrite) -> Result<Rev>; // R7
}
```

`DomainWrite` = envelope change + a list of `RowChange { table, op: Insert | Update | Delete, key, values: Vec<(col, Typed)> }`
+ the activity (kind, actor, target). `Typed` is an enum of the logical types (no JSON, no `Any`). The `Store` facade keeps the
v4 traits for programs that have not moved (ADR 0045 "both offered"): a v5 store opened by v4 code is refused (as 0.3 refuses a 4
file), a v4 file is opened read-only by v5 code for the migration only.

## 7. Migration sketch v4 -> v5 (no code yet)

Goals: nothing is lost silently, the old file stays until a human deletes it, the run can be repeated.

1. **Detect and back up.** `schema_version = 4` found: copy the file next to itself (`<name>.v4.bak`), refuse if the copy cannot be
   written. v5 is built into a **new file** (D5); the v4 file is opened read-only.
2. **Envelope.** Per `objects` row: `type`, `name`, `attributed_to` (text -> `PERSON`: the IRI as written, `art` looked up through
   the domain or `person`), `published`/`updated` (v4 microseconds -> `UTC` ms, **rounding recorded**), `context`, `in_reply_to`,
   `describes`. `rev` starts at 1. `iri` kept. For an imported object (`object_origin` written from the v4 origin data) `rev_at_import` is set **after** the domain step: `1` if the v4 side says "unchanged since import" (so `rev = rev_at_import`), otherwise `0` (so it counts as changed); an unknown state counts as changed. Otherwise "unchanged since import" would be lost.
3. **Domain content.** The registered `V4Migrator` of the object's `type` reads `content` (JSON, domain standard) and writes the
   typed rows through `DomainWrite`; the calendar's version normalizes like an import (floating time -> zone of the person + `zone_assumed`, all-day with a zone -> zone dropped + note, participants with name+e-mail -> `PersonDirectory`, one collective proposal) as `PLAN.md` 2.4 lists.
4. **Addressing, protection.** `addressing` JSON -> `object_addressing` rows; `sensitive` JSON and the `freesynergy.net:sensitive` key -> `object_protection`.
5. **Original.** `original`/`original_type` -> `object_original` (bytes as they are; `sha256` computed; `keep_until` from the retention rule of the kind, **[not checked]** where that rule lives).
6. **Activities.** `payload` `ChangeSet{before, after}` -> `activity` + `activity_change`: the domain migrator explains the diff of two images as typed field changes (`explain(before, after) -> Vec<FieldChange>`). Unknown type: activity row without changes (D7). **Deleted but not purged v4 objects** (the row is gone in v4, only the delete activity with its before image is left; `purged_objects` lists those that were purged and whose activities are gone): the activity needs an `objects` row because of the FK, so the migrator creates a **trash row** from the before image (`deleted` = time of the delete activity, `purge_after` from the retention rule, domain rows from the image through the domain migrator). A type without a migrator gets an envelope-only trash row (`id`, `type`, `name`, `deleted`) from the image, or the object is held (step 7), and every such object is listed in the report. An activity of an object that is neither in `objects`, nor in `purged_objects`, nor explained by a delete before image is held and reported, never inserted without a parent. `redacted`, `synced`, `purged_objects`, `unlogged_ids`, `meta` and the store identity are copied unchanged; the activity id order is kept (monotonic ids, ADR 0020).
7. **Unknown fields.** A field the typed tables cannot hold and that is not in the original: the object is **held** (not migrated), listed in the report, run continues; resolved by a human (ADR 0086 item 5, plan O6).
8. **Round trip per object** (red = the object is not migrated): v4 JSON -> typed rows -> export through the domain's boundary adapter -> compare with the v4 JSON **after the documented normalization**. Unchanged received data: export = `object_original` byte for byte.
9. **Resumable.** A table `fs_migration(source_sha256, last_object REF, state ENUM(running, held, done), at UTC)` in the new file, itself declared in `fs_column` (2.2) and `STRICT`, so R14 does not flag it; a restart continues after `last_object`; changed `source_sha256` aborts.
10. **Report** (`MigrationReport`): counts per type; per object: migrated / migrated with normalization notes / held; losses; people proposals. Written as a file next to the store and returned to the caller; **never** only logged.
11. **Swap** only after the person confirms: the v5 file becomes the store, the v4 file and its `.bak` stay.
12. **Dependents.** `rev` changes meaning (counter instead of hash): frontends and caches re-read after the migration. Programs that read `start_time`/`end_time` of the envelope (fs-calendar) change in their own migration step.

## 8. What the engine does, measured

`crates/fs-objects/tests/v5_probe.rs`, `v5_probe_keys.rs` and `v5_probe_delete.rs` (turso `=0.8.3`; in-memory connections, one probe on a file in the
cargo target tmp dir). The probes pin today's behaviour; a flip means this section and R4/R5/R16 are re-read.

| Probe | Result |
|---|---|
| `INTEGER` full i64, ORDER BY and range as numbers | works |
| `CHECK (...)` in `CREATE TABLE` | accepted and enforced |
| `CREATE UNIQUE INDEX ... WHERE ...` (partial) | accepted and enforced |
| Foreign key, `ON DELETE CASCADE` | works **after `PRAGMA foreign_keys = ON`**; without it orphan rows are accepted (off by default) |
| One transaction over several tables, `ROLLBACK` | works; also after a failed statement (CHECK, FK) inside the transaction: `ROLLBACK` leaves nothing and the connection stays usable |
| FK and partial `UNIQUE` on `TEXT` (UUID) keys | work (orphan refused, second master refused, same `(master, recurrence)` refused) |
| Self-referencing FK (`master -> appointment`, `ON DELETE CASCADE`), cascade over four levels (objects -> appointment -> participant -> alert, composite FK) | works; envelope rows of the exceptions are not removed (code does, R4) |
| `PRAGMA foreign_keys` on a second connection of the same file, and after reopening the file | **off** on both: the pragma is per connection, not stored; the store sets it on every connection. [not checked: encrypted file] |
| `STRICT` together with `CHECK` (`IN`, composite `OR`), FK and partial unique index in one table | all enforced |
| `activity -> objects` and `activity_change -> activity` with `ON DELETE CASCADE` | purge (`DELETE`) removes both; trash (`UPDATE deleted`) keeps them |
| `DELETE` + `INSERT` and `INSERT OR REPLACE` on a row with FK children (`v5_probe_delete.rs`) | both fire the cascade and wipe activity and changes; `UPDATE` and upsert (`ON CONFLICT DO UPDATE`) keep them: never write `objects` the v4 way |
| Activity of a deleted-not-purged v4 object | refused by the FK without an `objects` row; accepted with the trash row; restore keeps it |
| `NaN` / infinity in a `REAL` column | NaN becomes `NULL` (CHECK does not see it), infinity is stored: the layer refuses both |
| Composite primary key, `CHECK (x IN (..))` as ENUM, `ALTER TABLE ADD COLUMN ... DEFAULT` | work |
| `STRICT` tables | accepted; `TEXT` into `INTEGER` is refused; a plain table accepts it |
| Generated columns | refused (experimental flag) |
| `ALTER TABLE DROP COLUMN`/`RENAME`, triggers, expression indexes, `EXPLAIN QUERY PLAN` | **[not checked]** |

## 9. Reviewer checklist

1. O5 (section 2.1): agree `INTEGER` for `LOCAL` and `DATE` in all programs, with the registry doing the readable rendering?
2. D1: one `jscal_*` family or two copies; D2: drop the envelope times.
3. The exception list (2.4) is complete: `object_original.bytes`, `CIPHER`; nothing else needs a BLOB or JSON.
4. R4: the store, not the programs, owns `PRAGMA foreign_keys = ON` on every connection (probe shows the default is off).
5. R14 is a real gate: can it fail on a JSON-in-TEXT column? (heuristic only, see R14).
6. Delete is always trash in v5, `DELETE` only in purge, no `DELETE`+`INSERT`/`REPLACE` on rows with children (R4); and v4 objects that are deleted but not purged come over as trash rows (section 7 step 6): agreed?
6a. Section 7 step 6 (activities): is converting images to typed changes worth it for objects whose domain has no migrator (D7)?
7. `meta.retention` is JSON in v4 (section 3): typed table in v5, agreed?
8. `WHEN` hull offsets (2.3): +-16 h taken from the tz scan (historical local mean times); agree, and add the build-time zone test.
9. Build checklist, first items: (a) test that every zone of the tz database lies inside +-16 h (2.3); (b) NaN/inf refused on `REAL` (2.1); (c) history survives updates (R4).
10. The tasks list (section 4) is enough for a second user to test the contract (ADR 0045).
