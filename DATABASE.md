# Quorum — Local Storage

Rules for Quorum's on-disk state: the dismissals store
`.quorum/dismissals.sqlite` and the managed-section write to
`.quorum/conventions.md`. The schema DDL is
`crates/quorum-core/src/memory/schema.rs`. Service behaviour (pragmas,
the promotion state machine, file-then-SQLite ordering) is SERVICES.md
§6 and §6.1. This file records how the store behaves when several
processes use it at once, and what a returned `Ok` actually guarantees.

---

## Concurrency and migration

Several processes can open the store at once: a pre-push hook and a
manual `quorum review`, two terminals, or both on a fresh clone.

- **A write transaction never begins with a read.** SQLite does not call
  the busy handler when a connection that holds a read transaction
  upgrades to a write; it returns `SQLITE_BUSY` at once. So
  `busy_timeout = 5000` helps only a transaction whose first lock is the
  write lock. The store sets it as its first pragma, though rusqlite
  0.32 already sets the same value inside `Connection::open`. The
  migration begins `IMMEDIATE`. `record_seen`, `commit_promote` and
  `commit_demote` begin DEFERRED, but their first statement is an
  `UPDATE`, which takes the write lock straight from no lock. A new
  write path must do one or the other.
- **The WAL conversion is retried by hand.** On a fresh file,
  `PRAGMA journal_mode = WAL` opens a read and upgrades it internally,
  so it hits the same immediate `SQLITE_BUSY`. It is retried every
  10 ms up to the 5 s busy timeout. Once the file is in WAL mode the
  pragma is a no-op.
- **The migration probes the version inside its own transaction.** The
  probe used to run before the transaction, so two first runs could
  both see an empty `schema_version`, both migrate, and the loser failed
  on the primary key. The version row is now inserted with `OR IGNORE`
  as well.
- **`schema_version` holds exactly one row, enforced on read.** Its DDL
  cannot express that. `schema::read_schema_version` reads `COUNT(*)`
  and `MAX(version)` and refuses more than one row with
  `MemoryError::SchemaVersionRows`, naming the file, before the
  migration writes anything. The migration dispatch and the
  forward-compat check both read through it, so a newer binary that
  appended its version row could not slip past the forward-compat gate.
  Migrations must therefore keep bumping the one row in place: an
  appended row makes every binary refuse the store, including the one
  that appended it.

## Durability

- **SQLite commits.** The store runs `synchronous = FULL`. In WAL mode
  that fsyncs the WAL at every commit, so a committed dismissal,
  promotion or demotion survives power loss and not just a process
  crash. `NORMAL`, used before DL-1, synced only at checkpoint. In the
  `journal_mode = DELETE` fallback, FULL syncs the rollback journal and
  the database file at commit.
- **`conventions.md` writes** (`conventions::atomic_write`). The temp
  file is fsynced before the rename; a failure removes it and changes
  nothing. On Unix the parent directory is fsynced after the rename, so
  the rename itself is durable on ext4. A directory-fsync failure is
  returned, so the caller skips its SQLite commit and the file is left
  ahead of SQLite, the drift AC 175 already handles. A filesystem that
  cannot sync a directory at all (`EINVAL` / `ENOTSUP`, some network
  and FUSE mounts) is treated as synced, as SQLite itself does.

### Residual power-loss behaviour

- **Windows does not force the rename to disk.** There is no directory
  fsync on Windows, and `std::fs::rename` does not request
  `MOVEFILE_WRITE_THROUGH`. NTFS journals the rename but may flush that
  journal after the SQLite commit has reached disk. Power loss in that
  window can leave SQLite *ahead* of the file: a row says
  `promoted_convention` while `conventions.md` lacks its block, or a
  demote's row is gone while the block remains. This is the inverse of
  the drift AC 175 designs for. `quorum convention list --orphans`
  reports both directions (`db_orphans` and `file_orphans`).
- **AC 175's crash test covers a process crash, not power loss.** Its
  seam panics between the rename and the SQLite commit. Nothing in the
  suite simulates power loss; the guarantees above follow from the
  syscalls made, not from a test.
- **Disks that acknowledge a flush before the data is stable** (a
  volatile write cache without barriers or FUA) void all of the above.
  That is outside Quorum's control.
- **`.gitignore` appends are not fsynced.** Losing one costs only a
  repeat of the append on the next open.
