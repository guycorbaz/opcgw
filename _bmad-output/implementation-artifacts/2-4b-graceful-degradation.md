# Story 2-4b: Graceful Degradation (Missing or Corrupted Database)

Status: ready-for-dev

## Story

As an **operator**,
I want the gateway to start cleanly even if the database is missing or corrupted,
So that brief data loss is better than a complete failure.

## Acceptance Criteria

1. **Given** missing database file, **When** gateway starts, **Then** new database is created, gateway starts with empty state (no error).
2. **Given** corrupted database, **When** gateway detects corruption, **Then** it attempts repair via PRAGMA integrity_check.
3. **Given** repair failure, **When** repair doesn't succeed, **Then** corrupted file is deleted, fresh database created, gateway starts.
4. **Given** all scenarios, **When** gateway starts, **Then** it logs what happened at info level and continues.
5. **Given** disk or permission issues, **When** errors occur, **Then** they are logged and handled gracefully (no panic).

## Tasks / Subtasks

- [ ] Task 1: Handle missing database (AC: #1)
  - [ ] SqliteBackend::new() tries to open file
  - [ ] If file doesn't exist: SQLite auto-creates it
  - [ ] schema auto-creation (from 2-2b) runs
  - [ ] Gateway starts with empty state (OK)

- [ ] Task 2: Detect corruption (AC: #2)
  - [ ] After opening connection, run PRAGMA integrity_check
  - [ ] If returns "ok": proceed normally
  - [ ] If returns errors: attempt repair

- [ ] Task 3: Repair logic (AC: #2, #3)
  - [ ] If integrity_check fails:
    - Try PRAGMA integrity_check or REINDEX
    - If still fails: delete corrupted file
    - Create fresh database via schema auto-creation
  - [ ] Log action taken

- [ ] Task 4: Error handling (AC: #4, #5)
  - [ ] Catch rusqlite::Error variants:
    - DatabaseCorrupt → attempt repair
    - DatabaseLocked → retry with backoff
    - PermissionDenied → log and fail gracefully
    - DiskFull → log and fail gracefully
  - [ ] Log at info level: "Database [status]"

- [ ] Task 5: Integration tests (AC: #1, #2, #3)
  - [ ] Test: missing DB file → created fresh
  - [ ] Test: corrupted DB → repair attempted, fresh DB created
  - [ ] Test: DB with permission denied → logged, starts anyway (if possible)
  - [ ] Test: all scenarios gateway starts without panic

- [ ] Task 6: Build, test, lint
  - [ ] `cargo build` — zero errors
  - [ ] `cargo test` — all tests pass
  - [ ] `cargo clippy` — zero warnings

## Dev Notes

### Corruption Detection

PRAGMA integrity_check returns:
- "ok" — no issues
- List of errors — corruption detected

If not "ok", attempt repair. If still fails, delete and recreate.

### Error Recovery Priority

1. Missing file → auto-create ✅
2. Corrupted → repair attempt
3. Permission denied → log, continue with what's available
4. Disk full → log, fail gracefully (don't panic)

Goal: gateway always starts, even if data is lost.

### What NOT to Do

- Do NOT attempt to fix individual corrupted tables
- Do NOT require manual intervention
- Do NOT panic on database errors
- Do NOT expose database-level errors to user (translate to friendly messages)

## File List

- `src/storage/sqlite.rs` — update new() with repair logic
