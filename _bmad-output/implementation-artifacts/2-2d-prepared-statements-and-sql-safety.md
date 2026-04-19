# Story 2-2d: Prepared Statements & SQL Injection Prevention

Status: ready-for-dev

## Story

As a **developer**,
I want safe, efficient SQL queries using prepared statements,
So that SQL injection is prevented and performance is optimized.

## Acceptance Criteria

1. **Given** SQL injection risk, **When** parameters are bound, **Then** all queries use prepared statements with parameterized values.
2. **Given** repeated queries, **When** statements are executed, **Then** prepared statements are cached to avoid recompilation.
3. **Given** common operations, **When** CRUD methods run, **Then** all use cached prepared statements via params![] macro.
4. **Given** SQL safety, **When** queries are inspected, **Then** no format!() or string concatenation in SQL.
5. **Given** performance requirements, **When** prepared statements are cached, **Then** metric operations are faster than 2-2c baseline.

## Tasks / Subtasks

- [ ] Task 1: Create PreparedStatements struct (AC: #1, #2)
  - [ ] Define struct holding all statement handles
  - [ ] Statements needed:
    - SELECT metric_values WHERE device_id=? AND metric_name=?
    - INSERT OR REPLACE INTO metric_values (...)
    - SELECT * FROM command_queue WHERE status=? ORDER BY created_at
    - UPDATE command_queue SET status=? WHERE id=?
    - SELECT/INSERT into gateway_status
  - [ ] Constructor: `fn new(conn: &Connection) -> Result<Self>`

- [ ] Task 2: Refactor SqliteBackend CRUD (AC: #3, #4)
  - [ ] Update `new()` to create and store PreparedStatements
  - [ ] Update `get_metric()` to use cached prepared statement
  - [ ] Update `set_metric()` to use cached prepared statement
  - [ ] Update `queue_command()` to use cached prepared statement
  - [ ] Update `get_pending_commands()` to use cached prepared statement
  - [ ] Update `update_command_status()` to use cached prepared statement
  - [ ] Verify all use params![] macro, no string concatenation

- [ ] Task 3: Audit for SQL injection (AC: #4)
  - [ ] Code review: grep for format!(), concat(), string interpolation
  - [ ] Verify all user-controlled values are bound parameters
  - [ ] Verify table/column names are hardcoded (never user input)

- [ ] Task 4: Performance validation (AC: #5)
  - [ ] Benchmark: 100 metric sets with prepared vs. compiled each time
  - [ ] Assert prepared statements are faster
  - [ ] Test: statement reuse doesn't cause stale state

- [ ] Task 5: Update tests (AC: #3)
  - [ ] All existing 2-2c tests should pass unchanged
  - [ ] No additional test changes needed (refactoring only)

- [ ] Task 6: Build, test, lint
  - [ ] `cargo build` — zero errors
  - [ ] `cargo test` — all tests pass
  - [ ] `cargo clippy` — zero warnings

## Dev Notes

### PreparedStatements Pattern

```rust
pub struct PreparedStatements<'conn> {
    select_metric: Statement<'conn>,
    upsert_metric: Statement<'conn>,
    select_pending_commands: Statement<'conn>,
    update_command_status: Statement<'conn>,
    // ... others
}

impl<'conn> PreparedStatements<'conn> {
    pub fn new(conn: &'conn Connection) -> Result<Self> {
        Ok(Self {
            select_metric: conn.prepare("SELECT ...")?,
            // ...
        })
    }
}
```

Lifetime tied to Connection to ensure statements don't outlive it.

### Binding Values

Always use `params![]` macro from rusqlite:
```rust
stmt.query_row(params![device_id, metric_name], |row| { ... })
```

Never: `format!("... '{}'", user_input)`

### What NOT to Do

- Do NOT use rusqlite::OptionalExtension without clear error handling
- Do NOT create new statements in loops
- Do NOT assume parameter ordering — always use named params if possible

## File List

- `src/storage/sqlite.rs` — update with PreparedStatements refactoring
