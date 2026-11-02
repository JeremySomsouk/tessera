# Terminal performance and capacity

This document records the terminal architecture audit against baseline commit
`41d8c2b`, plus the changes and measurements made during this work. Baseline line
references below deliberately refer to that commit, so they remain reproducible
when implementation changes move functions. Use `git show 41d8c2b:src/terminal.rs`
to inspect the original. Dependency references identify the exact locked crate
and its source file, available under Cargo's registry source directory.

## Data path and ownership at the baseline

| Stage | Execution and data ownership | Contention, cancellation and lifetime |
| --- | --- | --- |
| Spawn | `Terminal::spawn_command_with_cache`, `src/terminal.rs:420`, opens native PTY, sets working directory/environment, spawns login shell or agent, clones the reader and takes the writer. Initial geometry is 100 by 30. No socket/JSON transport is used for terminal bytes. | Spawn itself is synchronous. Ordinary workspace creation and fresh-shell restoration call it on the UI thread (`src/ui.rs:683`, `src/ui/restoration.rs:199`). Agent validation runs separately, but successful restored agent spawn is again on the UI thread (`src/ui/restoration.rs:257`). |
| Keyboard | egui events become small `Vec<u8>` values through terminal key/mouse encoding (`src/ui.rs:5027`). `input` / `input_at_cursor` move those vectors into the command channel (`src/terminal.rs:682`). | Nonblocking admission returns an explicit error if the queue is full. Admission is not an acknowledgement that the child consumed the bytes. |
| Paste | OS clipboard backend returns an owned UTF-8 `String`; egui passes `Event::Paste`. `Terminal::paste`, `src/terminal.rs:713`, removes ESC, then allocates another string for bracket framing or LF-to-CR conversion. | Bracketed mode preserves LF and emits one begin/end pair. Unbracketed paste changes LF to CR. Clipboard access is user initiated. In-flight paste originally has no separate cancellation operation. |
| PTY writes | One host thread receives input, resize, scroll and keyboard-selection commands (`src/terminal.rs:512`). It calls blocking `write_all`; user input scrolls to bottom under the grid lock first. | A child which stops reading can block the host thread, including selection and resize processing. Up to 256 commands queue behind the write. Bytes are not serialized or decoded on this route. |
| PTY reads | A separate reader thread owns one 16 KiB stack buffer and one persistent VTE processor (`src/terminal.rs:467`). It reads raw bytes and calls `Processor::advance` under `Arc<Mutex<Term<Listener>>>`. | This is naturally backpressured: while parsing or waiting for the grid, no next read occurs and the OS PTY buffer eventually blocks the producer. No unbounded output channel is present. UTF-8 and ANSI state survive read boundaries. |
| Protocol replies | Parser events are collected in `Arc<Mutex<Vec<Event>>>`, converted into byte vectors under the grid lock, then synchronously sent to the host channel (`src/terminal.rs:478`). | A blocked writer plus a full reply queue can stop output draining. OSC 52 clipboard events are deliberately ignored by the listener (`src/terminal.rs:179`). |
| Grid/history | Alacritty stores decoded Unicode cells in a ring of rows. Primary screen retains 10,000 history rows; alternate screen has no history. Ring rotation reuses rows rather than shifting the entire scrollback (`alacritty_terminal 0.25.1`, `src/grid/storage.rs:15`, `src/term/mod.rs:414`). | Parser, UI, search, snapshot and selection share one mutex. Ordinary text memory is proportional to columns times retained rows. Row limits are not a strict byte bound because cells may contain variable-length combining marks/hyperlinks. |
| Rendering | UI uses `try_lock`, iterates visible cells, resolves styles and issues per-cell egui text/shape calls (`src/ui.rs:4503`). It clones generated shapes into a fallback cache. On contention it draws that cache and requests a retry in 16 ms. | Rendering does not wait for the parser, but successful rendering holds the mutex during layout/painting. Only visible cells are rendered. Baseline extracts and clones the complete selected text on every successful frame, even when no copy is requested (`src/ui.rs:4804`). |
| Search | Literal case-sensitive query compiles to the engine's regex search. At most one worker per pane locks the grid and searches retained history (`src/search.rs:102`, `src/search.rs:199`). Edits coalesce and generations reject stale results. | Search avoids a full history clone and UI wait, but the scan holds the parser's grid lock. Closing a search does not interrupt an already-running scan. The bound is retained grid size, not an elapsed-time budget. |
| Copy | Alacritty `selection_to_string` walks the selected rows and constructs a UTF-8 string, omitting wide spacer cells and joining wrapped lines (`alacritty_terminal 0.25.1`, `src/term/mod.rs:529`). Keyboard copy executes on the host worker; mouse/context-menu copy uses UI selection text and `Context::copy_text`. | Full selected text must fit in memory. egui queues an output command, then egui-winit transfers the complete string to its OS clipboard backend. There is no app-level copy byte limit. The OS transfer itself is not measured by headless egui tests. |
| Periodic persistence | UI captures directory/conversation metadata and rendered text tails, merges prior restored history with current output, clones `Saved`, and uses a capacity-one save channel (`src/ui.rs:607`, `src/ui.rs:3865`, `src/ui/restoration.rs:63`). Background writer serializes JSON, validates previous generation, writes backup and primary via private temp files, fsyncs and renames (`src/snapshot.rs:54`). | UI uses `try_lock` for normal capture and keeps prior output if busy. Disk I/O is off the UI thread, but capture, string copies and `Saved::clone` are not. Baseline can capture twice in the same periodic-save frame. Full save-channel retries still reconstruct/clone data. |
| Restoration | Startup reads/decodes one workspace JSON with backup fallback (`src/ui.rs:590`, `src/snapshot.rs:40`). Prior output is shown in a separate selectable read-only panel, never replayed through ANSI (`src/ui/restoration.rs:144`, `src/ui/restoration.rs:329`). New shells start or validated agents resume exact conversation IDs. | This is restoration of workspace metadata, recent rendered output and agent conversation identity, not a persistent PTY or process checkpoint. Remote-terminal output is excluded. Resume errors retain identity/output and open a fresh shell. |
| Exit/cleanup | Baseline terminal drop sets stopping, invokes the child killer, tries to enqueue Stop, then joins host (`src/terminal.rs:739`). Host waits/reaps child and removes private remote cache. Reader is detached and exits on EOF/error. | Blocking write or surviving descendant PTY handles can delay cleanup. Child exit and PTY-drain completion are distinct events; the baseline shares an `alive` flag between threads. App shutdown captures with blocking grid locks, enqueues final save, and joins persistence (`src/ui.rs:3886`), favoring durability over bounded shutdown time. |

## Limits inventory

These are capacity and semantic boundaries, not promises of unlimited capacity.
The implemented-change section identifies which baseline bottlenecks changed.

| Limit or policy | Baseline source | Classification and user consequence |
| --- | --- | --- |
| 64 KiB per input vector, including paste framing | `src/terminal.rs:688` | Arbitrary bottleneck. A large paste is rejected in full, including some pastes whose text alone is just below 64 KiB. Not an OS PTY limit. |
| 256 host commands | `src/terminal.rs:447` | Resource safeguard measured in events, not bytes. With the input cap it can retain roughly 16 MiB of user payload per pane, plus in-flight data and protocol replies. UI reports full queue; parser replies block. |
| 16 KiB PTY read batch | `src/terminal.rs:469` | Working-buffer size, not output truncation. Larger read batches can improve throughput but hold the grid longer. |
| 250 ms host receive timeout | `src/terminal.rs:525` | Child-status/stop polling interval while idle. Not an input write timeout. |
| 10,000 history rows | `src/terminal.rs:96` | Fixed retention default/safeguard. Older rows are evicted. Huge unbroken lines wrap into rows and consume this retention rapidly; logical line count is not the bound. Not user-configurable at baseline. |
| 1,000 extra cached rows | `alacritty_terminal 0.25.1`, `src/grid/storage.rs:13`, `:111`, `:133` | Allocation optimization. Retained allocation can exceed active grid/history row count. History grows lazily. |
| Alternate-screen history = 0 | `alacritty_terminal 0.25.1`, `src/term/mod.rs:416` | Terminal semantics. Full-screen programs own their screen, not primary history. |
| 500 columns / 200 visible rows | `src/ui.rs:4540` | UI resource guard. Very large windows or tiny fonts stop increasing terminal geometry at this bound. |
| `u16` PTY rows/columns | `src/terminal.rs:84`; `portable-pty 0.9.0`, `src/lib.rs` (`PtySize`) and `src/unix.rs:181` | External API representation. UI bounds are much smaller. Kernel PTY queues, process/file-descriptor quotas and line-discipline limits vary by platform and terminal mode. |
| 32 workspaces/live panes | `src/ui.rs:758`, `:786`, `:824`, `:3045`, `:3098`, `:3317` | Fixed application safeguard. Creation is refused with a visible error. Limits CPU, grids, processes, threads and descriptors. Restoring a hand-edited snapshot does not validate this count at load time. |
| 256 tracked agent sessions | `src/ui.rs:1048`; `src/discovery.rs:134` | Metadata retention safeguard. Ended/disconnected records are evicted first; all-active overflow reports an error on hooks and skips new discovery records. Distinct from live terminal count. |
| 256 search-query characters | `src/search.rs:174` | UI/compiler resource safeguard; literal queries only. Search covers retained output, not discarded history or prior-run saved output. |
| 128 pending wheel steps | `src/ui.rs:5283` | Interaction coalescing/clamping, not output loss. Prevents a single wheel burst from accumulating unbounded navigation work. |
| 256 KiB / 2,000 rendered rows saved per pane | `src/terminal.rs:109`; `src/ui/restoration.rs:32` | Deliberate retention. Newest text is kept at UTF-8 boundaries, trailing spaces/control characters/hidden text are excluded. Saved output is a lossy visual tail, never a byte-perfect transcript. |
| 32 MiB workspace snapshot | `src/snapshot.rs:10`, `:25`, `:84` | Persistence safeguard. Oversized saves fail without replacing valid state. Baseline checks serialized size after allocating full JSON. Load bounds file bytes before decoding, but decoded memory can exceed file size. |
| One queued save plus active save | `src/ui.rs:607` | Bounded queue. Full queue retries after 100 ms; saves normally at least 2 s apart, automatic output capture every 5 s (`src/ui.rs:3865`). Shutdown waits for final durable write without a deadline. |
| Backup + temp-file generations | `src/snapshot.rs:54`, `:88` | Disk durability trade-off. Primary/backup plus an in-progress temp can use roughly three snapshot sizes; crash-left temp files are ignored. Snapshot permissions are 0600 on Unix. |
| Resume verification 3 s; 20 ms poll; 100 ms pipe completion | `src/resume.rs:134`, `:139`, `:161`, `:172` | Bounded CLI capability checking. Slow/broken CLI yields retryable error. This does not timeout the actual long-running agent session. |
| Resume help stdout/stderr 256 KiB each; errors 4,096 chars | `src/resume.rs:115`, `:147` | Diagnostic retention only. Pipe is drained after cap to avoid deadlock; excess help text is not retained. |
| Session metadata inspection 1 MiB / 128 lines | `src/resume.rs:203` | Bounded conversation validation, not terminal output cap. Metadata beyond that window cannot validate the session. |
| Discovery every 2 s, request/result queues of one | `src/discovery.rs:19`, `:52` | Coalesces process scans. Tracking may lag process starts/exits; unrelated to PTY throughput. |

### Side-channel IPC and remote metadata

Agent hooks use a private Unix socket, not the terminal byte stream. Endpoint
directory mode 0700 and socket mode 0600 protect the boundary. The client reads
at most 64 KiB + 1 from stdin, allowlists metadata and serializes JSON; the server
processes one connection at a time, with a 500 ms read timeout, 64 KiB cap and a
256-event channel (`src/integration.rs:15`, `:34`, `:40`, `:81`). The sender has a
500 ms write timeout. Session IDs are bounded to 256 bytes, detail to 4,096 and
event names to 64 (`src/integration.rs:51`). Oversized/malformed server messages
are discarded; client validation reports errors. UI drains up to 256 events per
frame (`src/ui.rs:1002`). This metadata can lag or be rejected under overload
without truncating terminal output. Dropping Endpoint unlinks the socket but
does not join/wake a thread blocked in accept (`src/integration.rs:68`).

Remote log terminals execute a command inside the same PTY path, with a private
0700 temporary cache removed at terminal teardown (`src/terminal.rs:211`,
`:331`). Separate remote dashboard requests have a 20 s deadline and 8 MiB
response cap (`src/remote_runs.rs:606`), a global four-process gate, a 45 s / 16 MiB
cache and a 60 s secondary-rate-limit cooldown (`src/remote_runs/runtime.rs`).
The dashboard `run_limit` is configurable from 1 to 1,000 (`src/remote_runs.rs:94`).
These limits apply to dashboard metadata, not arbitrary bytes in a log terminal.

### Dependency and platform boundaries

- Locked versions inspected: `alacritty_terminal 0.25.1`, `vte 0.15.0`,
  `portable-pty 0.9.0`, `eframe/egui/egui-winit 0.33.3`, `arboard 3.6.1`.
- VTE keeps UTF-8 partial state across reads (`vte`, `src/lib.rs:65`). CSI has
  32 parameters/subparameters, 16-bit numeric parameters, two intermediates;
  OSC has 16 parameters (`src/params.rs:5`, `src/lib.rs:44`). Unsupported or
  overlong control commands need not have their intended effect. These are
  parser protocol bounds, not plain text truncation.
- VTE synchronized output has a 2 MiB buffer and 150 ms intended timeout
  (`vte`, `src/ansi.rs:35`, `:263`). The embedder must service the timeout and
  flush at EOF: `StdSyncHandler::pending_timeout` checks presence, not expiry
  (`:445`). The baseline reader did neither, so an incomplete synchronized
  update could remain invisible after idle/exit.
- VTE's often-quoted 1,024-byte OSC constant applies only without `std`.
  This application uses the `std` implementation, whose OSC raw storage is a
  growable `Vec` (`vte`, `src/lib.rs:48`). An unterminated huge OSC can therefore
  grow memory. OSC 52 denial prevents clipboard access but does not eliminate
  parser buffering.
- Combining marks have no per-cell byte cap: `Cell::push_zerowidth` appends to
  a vector (`alacritty_terminal`, `src/term/cell.rs:164`). A single cell with
  millions of combining marks defeats a row-count memory bound. Titles can
  also be large, and title/keyboard stacks permit 4,096 entries (`src/term/mod.rs:42`).
  Ordinary-output memory plateau measurements do not prove hostile-output
  memory safety. Silently clipping these values would change terminal semantics.
- Unix PTY reads/writes use cloned file descriptors. Linux EIO on closed slave
  is mapped to EOF (`portable-pty`, `src/unix.rs:94`). The writer destructor may
  write newline/EOT in canonical mode (`:393`). Stream byte boundaries are not
  Unicode boundaries, and a receiving application's decoder must be incremental.
- The OS terminal line discipline can transform bytes and reject/limit long
  canonical input lines. Exact transport tests must configure a raw receiver;
  byte-perfect delivery to a raw PTY does not promise shell/application acceptance.
  Tessera must not disable a user's line discipline globally to hide this limit.
- Clipboard transfers are whole-string operations in egui-winit
  (`egui-winit`, `src/clipboard.rs:57`, `:94`), through arboard on macOS/X11 or
  smithay on Wayland. OS/backend failures are logged; no fixed universal
  clipboard size guarantee exists. Arboard X11 uses a 4 s initial receive
  timeout and 10 ms incremental timeout (`arboard`, `src/platform/linux/x11.rs:101`,
  `:585`). macOS/Wayland behavior must be validated separately. Headless copy
  output tests verify Tessera's extraction and handoff, not another application's
  clipboard retrieval.

## Implemented changes

- `src/terminal.rs`: nonblocking native Unix PTY reads/writes, partial-write
  accounting and 16 KiB input processing steps. Controls are serviced between
  writes, so a process that stops reading no longer blocks resize, selection,
  or stop requests behind a blocking `write_all`.
- Ordered input admission uses both a 256-message queue and a byte budget:
  64 MiB of allocated payload capacity by default, configurable using a positive
  `TESSERA_INPUT_BUFFER_BYTES` value. Admission failure rejects the entire new
  input before writing any of it. The active input remains charged until fully
  written or cancelled. One queued paste remains an indivisible ordered item;
  neither later typing nor protocol replies appear inside its bracketed frame.
- Protocol replies have their own 256-message / 1 MiB reserve, independent of
  user input. User and reply queues alternate at whole-input boundaries when
  both have work, preventing reply traffic from starving typing. A process that floods requests without reading responses causes
  an explicit terminal error and cancellation instead of a cyclic deadlock.
  A normal reply still fits when the user input budget is full.
- Paste filtering and newline conversion happen incrementally on the worker.
  Working scratch is about 16 KiB, in addition to the accepted source string.
  Both source scanning and emitted bytes are bounded per step, including a paste
  containing only ESC bytes. ESC removal and bracketed-paste behavior are retained.
  Partial OS writes may split UTF-8 code points; byte order is preserved and
  decoding remains incremental. No new Unicode normalization is applied.
- VTE synchronization expiry is serviced even when no new output arrives, and
  buffered synchronized output flushes at EOF. Reaping a child no longer marks
  its still-draining output as complete. Closing a pane cancels pending I/O and
  joins both workers. On cancellation the host closes its writer/master handles
  before killing/reaping the child: macOS can otherwise leave a child exiting
  with unread PTY output while the host waits forever. The host alone owns
  killing/reaping, avoiding concurrent signals through a stale cloned PID.
  Directory capture returns the launch directory once that shell is reaped,
  even while its final output drains, instead of looking up a potentially reused PID.
  Input failure and incomplete delivery after exit are reported.
- Terminal repaint notifications coalesce per viewport/pass. egui otherwise
  appends a repaint-cause record for every request until a frame runs. This bounds
  request accumulation for an inactive UI while still promoting a scheduled
  delayed repaint to an immediate wake on output. Idle PTY polling requests at
  most a 250 ms wait, shortened for synchronization deadlines; native readiness
  wakes the reader when data arrives.
- `src/ui.rs`, `src/selection.rs`: selected history is no longer extracted and
  cloned on every frame. Keyboard copy extracts on the control worker and keeps
  the current terminal mode. Context menus capture once and share their text
  until an action is chosen; a busy grid asks the user to reopen the menu.
- `src/ui/render_budget.rs`: rendering retries share a 2 ms allowance across
  visible panes in the root frame, including immediate detached windows and egui
  discard passes. Each visible live pane reserves an equal share, capped at the
  prior 500 microsecond per-pane allowance. A busy early pane cannot spend later
  panes' reservations; repeated layout cannot refill them. Inactive workspaces,
  minimized windows and nonterminal views do not receive reservations. Immediate
  successful locks still paint after the retry budget is spent. Cached shapes
  remain the fallback. Actual elapsed retry time is charged, including scheduler
  delays, so this is not a hard real-time frame deadline. Painting and other UI
  work are outside the wait allowance. See the shared-wait measurements below.
- `src/ui/restoration.rs`: tail retention bounds the intermediate allocation
  before joining lines. `src/ui.rs` no longer captures every terminal twice in
  the same periodic-save frame. Retention semantics are unchanged.
- `src/snapshot.rs`: JSON serialization stops as soon as it would exceed 32 MiB,
  before materializing a potentially much larger encoded value. The previous
  snapshot and recovery copy remain untouched on rejection.

### Decisions and limits retained

The implementation keeps the existing PTY/parser/renderer abstractions. It uses
bounded memory rather than a new disk spool. A 64 MiB default allows substantially
larger pastes but is still a resource ceiling; 32 saturated terminals could retain
up to 2 GiB of input alone, plus reply reserves, grids, clipboard/event strings,
and application state. There is no new global memory governor. Raise the input
budget only with that aggregate cost in mind.

Accepted input is not an acknowledgement from the receiving program. Exact
regressions compare bytes at a raw PTY receiver, including Unicode, newline
conversion, escape filtering, bracketed framing and input following the paste.
Ctrl-C queues behind an already accepted paste. Closing its pane is the supported
cancellation boundary; cancellation may leave a prefix already received by the
process. No bytes are silently truncated to make an input fit.

History eviction remains explicit: 10,000 live rows, a 2,000-row / 256 KiB saved
tail, and a 32 MiB workspace snapshot. These are retained resource/retention
policies, not a complete transcript guarantee. The 32-pane cap remains. Neither
session retention nor canonical line-discipline limits were raised speculatively.
OSC 52 clipboard access remains denied. Saved text is never replayed as ANSI.

## Reproducing the measurements

Environment: macOS 26.7.1 (25G241), arm64, Rust 1.99.1
(58cf8f9c3, Homebrew), locked dependencies above. Host model/physical memory were
unavailable to this execution environment. PTY throughput, continuous output and
restoration measurements use Cargo's unoptimized test profile. Static selection
rendering uses the optimized release profile and is reported separately. No GPU
window or native clipboard transfer is included. The machine was not isolated
from other desktop work; the initial figures below are single-run observations,
not confidence intervals or service-level guarantees. The later shared-wait
follow-up reports three optimized repetitions separately.

The original source was archived from `41d8c2b8de9e73f5aaab897508dc1e72c6b720fa`
to a separate directory. Only benchmark modules/instrumentation and the Python
fixture were added to that copy. Baseline executables were copied before rebuilding
the changed source. This avoids mixing behavior changes into the baseline.
Other-chat autoscroll changes in the working tree were preserved.

The benchmark-only [baseline patch](benchmarks/terminal-2026-11-02/baseline-harness.patch)
is included so the original measurements can be reproduced without copying
changed production code. From this checkout:

```sh
TESSERA_CHECKOUT="$PWD"
mkdir -p /tmp/tessera-performance-baseline
git archive 41d8c2b8de9e73f5aaab897508dc1e72c6b720fa | tar -x -C /tmp/tessera-performance-baseline
patch -d /tmp/tessera-performance-baseline -p1 < "$TESSERA_CHECKOUT/docs/benchmarks/terminal-2026-11-02/baseline-harness.patch"
cd /tmp/tessera-performance-baseline
# Run the same benchmark commands below. Use this checkout's own target directory.
```

The baseline harness checks output completion markers; final code adds exact
PTY byte counters. Those assertions run outside timed output sections. The
benchmark-only baseline patch also adds test-only fresh/cached render counters,
without changing lock acquisition, parsing, or I/O behavior.

```sh
# Functional transport, Unicode, overload, cancellation and clipboard regressions
cargo test --locked terminal:: -- --test-threads=1
cargo test --locked large_copy_waits_for_busy_grid_and_delivers_exact_selection
cargo test --locked ui::restoration::tests

# Increase output gradually. Each run covers build, Unicode/ANSI agent and
# unbroken-line workloads at 100 columns x 30 rows, with 1 then 4 terminals.
TESSERA_BENCH_MIB=1 cargo test --locked performance::terminal_workloads -- --ignored --exact --nocapture
TESSERA_BENCH_MIB=4 cargo test --locked performance::terminal_workloads -- --ignored --exact --nocapture
TESSERA_BENCH_MIB=16 cargo test --locked performance::terminal_workloads -- --ignored --exact --nocapture

# One continuous 256 MiB Unicode/ANSI session, 512 MiB process RSS budget,
# 60-second output budget. This is a bounded soak, not an hours/days test.
TESSERA_BENCH_SOAK=1 cargo test --locked performance::terminal_soak -- --ignored --exact --nocapture

# Raw receiver: 32 KiB, 256 KiB, 4 MiB paste; paused-reader resize latency.
cargo test --locked transport_benchmark -- --ignored --nocapture --test-threads=1

# 16 MiB with actual headless terminal_view frames at a 16 ms cadence.
TESSERA_BENCH_RENDER=1 cargo test --locked benchmark_heavy_output_rendering -- --ignored --nocapture

# 16 restored panes, 4 MiB saved output; capture, save, backup, load, restore.
TESSERA_BENCH=1 cargo test --locked benchmark_restoration -- --ignored --nocapture

# 100 optimized headless frames, 960,199-byte selection, 900 x 480 viewport.
TESSERA_BENCH=1 cargo test --locked --release benchmark_terminal_rendering -- --ignored --nocapture
```

The Python fixture requires `python3`. Normal CI does not enable the opt-in
resource benchmarks. Output budgets are 1..16 MiB/session for the matrix,
30 seconds/workload and 180 seconds overall, with startup deadlines. The continuous
run samples CPU and process peak RSS once per second. Its input probe runs 10 ms
after output starts. The app has no active UI frames in the matrix/soak, deliberately
exercising background output and repaint accumulation. Heavy-render tests run
real egui frames separately, without native presentation.

`input_ack_ms` measures the time to observe a receiver acknowledgement file,
with 1 ms polling in the PTY matrix and one-frame polling in the render benchmark.
It includes scheduling and filesystem overhead. `busy_pct` samples grid-lock
contention; it is not frame latency. `max_without_fresh_ms` includes the normal
16 ms frame cadence and detects cached/stale output despite short frame execution.
CPU is `getrusage(RUSAGE_SELF)` time; peak RSS is the cumulative high-water mark,
not live retained heap or proof of a leak. Startup uses the configured login
shell, so local shell initialization is included. Restore benchmarks use clean
test shells and are not a measurement of Claude/Codex resume or network services.

Output marker completion establishes baseline ingestion. The changed harness
also asserts exact PTY read byte counts, and exact write counts for the input
probe. Paste tests independently compare the complete receiver file with the
expected transformed bytes. Copy tests compare the full egui `CopyText` payload;
OS clipboard acceptance and another application's paste are outside that boundary.

### Recorded results

Raw records are stored in [benchmarks/terminal-2026-11-02](benchmarks/terminal-2026-11-02/).
Representative measured results (latest serial runs):

| Boundary / workload | Baseline | Changed |
| --- | ---: | ---: |
| 32,765-byte paste, raw receiver completion | 15.650 ms | 18.210 ms |
| 262,140-byte paste | Rejected | Exact receipt, 31.123 ms |
| 4,194,300-byte paste | Rejected | Exact receipt, 225.395 ms |
| 4 MiB paste submission | 26.098 ms, rejected | 0.318 ms, admitted |
| Resize while input consumer pauses for 1 s | 1,072.166 ms | 5.811 ms |
| 960,199-byte selection, release frame median / p95 | 2.704 / 5.277 ms | 0.316 / 0.432 ms |
| Same selection, maximum of 100 frames | 15.537 ms | 0.570 ms |
| Build output, fresh headless render passes | 7.5% | 100% |
| ANSI/Unicode agent output, fresh render passes | 7.3% | 98.2% |
| Build output, longest interval without fresh paint | 598.336 ms | 19.633 ms |
| Agent output, longest interval without fresh paint | 692.691 ms | 35.606 ms |
| Build frames, median / p95 / max | 0.740 / 2.073 / 3.601 ms | 1.889 / 2.188 / 4.093 ms |
| Agent frames, median / p95 / max | 0.321 / 0.914 / 3.448 ms | 0.890 / 1.104 / 3.495 ms |
| 16 MiB build with rendering, completion | 985.308 ms | 1,093.793 ms |
| 16 MiB agent with rendering, completion | 1,029.571 ms | 1,065.511 ms |
| Input acknowledgement during build / agent rendering | 20.646 / 20.588 ms | 18.960 / 17.586 ms |
| 256 MiB continuous output, completion | 44.104 s | 44.577 s |
| Continuous output, process peak RSS at 1 s / 44 s | 44.76 / 59.46 MB | 44.61 / 44.63 MB |
| Continuous output, process CPU at 44 s | 28.390 s | 29.061 s |
| 16-pane snapshot capture, median of 5 | 74.569 ms | 68.967 ms |
| 16-pane snapshot first save / save with backup | 98.792 / 127.980 ms | 100.000 / 124.507 ms |
| 4,239,979-byte snapshot load | 19.120 ms | 17.582 ms |
| Restore 16 clean shells and retained output | 590.149 ms | 606.691 ms |

The rendering change explicitly trades work for freshness. Relative to the
original baseline, the final build-render sample took about 11.0% longer and the
agent sample about 3.5% longer. Three intermediate paired retry measurements
showed roughly 6–7% build cost and 98–100% fresh frames; their raw logs are retained.
Frame execution remains short, but measuring only frame duration would have
missed the original stale-content problem. No small-paste or input-acknowledgement
speedup is claimed. The small persistence changes are within noisy single-run
variation; the removed duplicate capture and bounded allocation are structural
improvements, not evidence that restoration is now nonblocking.

Headless PTY ingestion, 16 MiB per session (no rendering):

| Workload | Sessions | Before MiB/s | After MiB/s |
| --- | ---: | ---: | ---: |
| Build records | 1 | 15.89 | 16.28 |
| Build records | 4 | 63.48 | 64.19 |
| ANSI/Unicode agent records | 1 | 15.31 | 15.35 |
| ANSI/Unicode agent records | 4 | 59.44 | 60.09 |
| One unbroken line | 1 | 16.28 | 16.45 |
| One unbroken line | 4 | 65.43 | 66.54 |

These small throughput differences do not establish a general speedup. The
16 MiB matrix processed 240 MiB in total across sequential scenarios: process
CPU was 16.654 s before and 16.574 s after; final peak RSS was 151.57 MB before
and 139.31 MB after. Receiver acknowledgement ranged 1.33–1.54 ms before and
1.16–1.53 ms after. Single-session login-shell startup ranged 1.61–1.62 s before
and 1.57–1.69 s after; starting four shells ranged 2.82–2.89 s before and
2.80–3.45 s after. Shell startup was not optimized. The 1 and 4 MiB matrix
records provide the smaller stress steps. Background `busy_pct` stays around
95–98% during the largest bursts; the renderer retry addresses fresh-frame
access rather than pretending the grid mutex is uncontended.

The 256 MiB run demonstrates that ordinary retained-output memory plus pending
repaint bookkeeping stayed stable for this roughly 44-second changed workload.
It does not establish an hours-long plateau or protection against unbounded
upstream OSC/combining-mark allocations. MB figures above use decimal bytes;
workload and capacity MiB use 1,048,576 bytes.

### Remaining risks to validate explicitly

1. A long search, copy extraction, resize/reflow or snapshot can hold the shared
   grid while output waits, even when no UI thread blocks on a mutex.
2. Application-level queues can be bounded while native clipboard allocation,
   VTE OSC storage and per-cell combining marks are not byte-bounded.
3. OS clipboard, GPU stalls, shell startup files, child process trees and
   platform-specific PTY behavior need real application testing. A headless
   synthetic benchmark alone cannot establish end-to-end responsiveness.
4. Retained history and saved history deliberately evict old text. For a durable,
   complete build/agent transcript, the producing tool must write a log file.
   This work does not add an unrestricted transcript database.


## Shared rendering wait follow-up

The initial retry change above used up to 500 microseconds per visible pane.
Sixteen busy grids could therefore spend 8 ms waiting before accounting for
painting. The follow-up uses one 2 ms root-frame budget with fixed fair shares,
while keeping the 500 microsecond single-pane ceiling and existing draw order.
This deliberately favors a predictable aggregate allowance over lending an idle
pane's reservation to an earlier busy pane. Immediate acquisitions never need
an allowance. Visibility changes during a frame may shrink later reservations;
the next root frame resets all shares. Current immediate child windows share
that root frame. A future move to deferred independent windows must revisit this
scheduler boundary.

The comparison baseline includes all earlier transport, copy and persistence
fixes. It isolates the per-pane versus shared wait policy, rather than comparing
against the original no-retry renderer. Three alternating optimized baseline and
changed runs use the same harness and fixture. The debug exploratory runs are
retained too: CPU saturation caused substantial variation and worse freshness
with the smaller allowance. They are not representative of the optimized app.

Each busy pane emits exactly 32 MiB of build-style ANSI/Unicode output, with
1, 4, 8 then 16 panes in a 1280 by 800 headless egui viewport at a nominal 16 ms
cadence. Additional cases hold the first of 16 busy grids for 500 ms, use only
one busy pane among 15 idle panes (first and last), and hold all 16 grids for
100 frames. Every output scenario checks exact PTY output counts, input
acknowledgement, nonterminal button clicks, and fresh paint recovery. The
all-held case checks button clicks and paint recovery. The
mixed scenarios focus and probe the busy pane; other scenarios probe the last.
Budgets are 60 seconds per output scenario, 240 seconds for the output suite,
and 1 GiB process RSS. No native event loop, GPU presentation or OS clipboard
is included. Frame duration covers terminal rendering and the probe button,
not every application panel.

Freshness means a successful grid acquisition and headless paint pass while
output is active. It does not mean changed pixels. Fresh rates are approximate:
the recovery frame can add one final fresh sample after the timed interval.
Stale intervals include the
normal frame cadence and host scheduling. Input acknowledgement is observed
once per frame; short cases have very few input/click samples. CPU timing
includes startup, warmup and teardown. The host was not isolated. OS, compiler,
commands and raw output are in
[the environment record](benchmarks/shared-render-wait-2026-11-02/release-environment.txt).

The following values are medians of the three per-run statistics, not pooled
percentiles. A stale value is the worst active pane in each run; the intentionally
held first pane is excluded in the held-first row.

| Scenario | Frame p95 before / after, ms | Worst stale before / after, ms | Input ACK p95 before / after, ms |
| --- | ---: | ---: | ---: |
| 1 busy | 0.481 / 0.650 | 20.184 / 20.327 | 20.352 / 20.350 |
| 4 busy | 0.798 / 0.731 | 20.749 / 20.347 | 20.792 / 20.681 |
| 8 busy | 0.990 / 1.262 | 22.463 / 39.642 | 20.987 / 20.905 |
| 16 busy | 17.666 / 13.066 | 47.482 / 61.400 | 38.188 / 32.707 |
| 16 busy, first grid held 500 ms | 17.402 / 11.828 | 60.212 / 55.162 | 44.858 / 35.944 |
| 16 visible, only first busy | 0.277 / 0.380 | 20.086 / 20.055 | 19.126 / 20.287 |
| 16 visible, only last busy | 0.302 / 0.315 | 20.114 / 20.137 | 20.301 / 20.310 |

With all 16 grids held, median frame execution fell from 8.198 to 2.208 ms;
all 100 button events per run still completed, and every pane painted fresh
immediately after release. This directly demonstrates the reduced aggregate
waiting cost. With 16 output-producing panes, the median click-frame p95 fell
from 12.265 to 10.798 ms and the fraction of frames above 16 ms fell from 6.4%
to 3.1%. Throughput was 121.98 versus 132.61 MiB/s. Across all three runs,
per-pane fresh acquisition rates remained 48.17 to 52.49 per second after the
change, versus 48.65 to 52.40 before. The mixed first/last cases were 100% fresh.

There is a measured freshness trade-off. For 16 busy panes, fresh-pass fractions
were 95.5% to 100% after versus 99.0% to 100% before. The median worst stale
interval increased by about 14 ms. For eight busy panes, frame p95 increased by
0.272 ms and the median worst stale interval rose from 22.463 to 39.642 ms.
The largest observed 16-pane input ACK was 57.677 ms after versus 45.091 ms
before, despite the lower median run maximum. These observations support the
2 ms fair budget for aggregate responsiveness, not a claim that every latency
or freshness metric improves. No starvation or recovery failures occurred.

Whole-suite median CPU time was 16.60 user + 98.85 system seconds before versus
15.79 + 85.06 after; elapsed time was 24.89 versus 23.61 seconds. These figures
include the test process and child work measured by the timing utility, not
isolated renderer CPU. Final process peak RSS was about 299 MB on both sides.
All six runs passed exact delivery, input, UI-event and recovery assertions.
The timing utility subsequently reported a denied hardware-clock query and
returned a nonzero status; its CPU fields and the harness's RSS measurements
were retained. No extra permission was requested solely for hardware metadata.
Raw runs and per-run ranges are in
[the benchmark directory](benchmarks/shared-render-wait-2026-11-02/).

To reproduce the changed measurements:

```sh
SHELL=/bin/sh TESSERA_BENCH_MULTIPANE=1 cargo test --release --locked \
  ui::render_benchmarks:: -- --ignored --nocapture --test-threads=1
```

To reconstruct the matching per-pane baseline, copy this working tree into a
separate disposable directory, excluding `.git` and `target`, then run there:

```sh
patch -p1 < docs/benchmarks/shared-render-wait-2026-11-02/per-pane-baseline.patch
SHELL=/bin/sh TESSERA_BENCH_MULTIPANE=1 cargo test --release --locked \
  ui::render_benchmarks:: -- --ignored --nocapture --test-threads=1
```

The patch restores only the pre-shared rendering integration and leaves the
identical benchmark harness in place. It was checked against the actual source
used for the recorded baseline. Use separate target directories or copy each
compiled executable immediately before building the other source tree.

## Validation record

The regressions cover atomic input admission and recovery, continuously available
protocol replies without starving user input, short/partial writes, escaped and
Unicode paste framing, exact multi-megabyte receipt followed by further input,
full user capacity plus a terminal query, nonreading reply consumers, repeated
cancellation with pending output, synchronized-output expiry/EOF, inactive repaint
coalescing, large exact copy through egui, copy-mode preservation, bounded render
fallback, UTF-8 retention, oversized serialization, and restored sessions.

Full local verification uses a clean shell and serial scheduling:

```sh
SHELL=/bin/sh cargo test --locked -- --include-ignored --test-threads=1
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 -m unittest discover -s tests -p 'test_*.py'
bash -n scripts/bundle-macos.sh scripts/prepare-sparkle.sh
sh -n install.sh scripts/package-linux.sh
```

The IPC and CLI round-trip tests require local Unix socket permission. The
initial parallel run hit that execution restriction; permitted socket tests
subsequently passed. Default-parallel runs also exceeded shell startup deadlines
on this host. An unchanged-baseline parallel run reproduced seven startup
failures. Personal zsh initialization ran dctl/oh-my-zsh setup, and concurrent
fixture launches competed for the short startup budgets. No production shell
policy or existing CLI-verification timeout was relaxed to hide this issue.
The new synchronized-output test uses a sleeping child that outlives its assertion
budget, so it still verifies a flush before EOF under slower scheduling.

Parallel stress also exposed a real macOS shutdown hang in the intermediate
implementation. A process stack sample showed the host in `child.wait()` while
holding its PTY handles and lifecycle mutex, with the flood producer already in
an exiting state. Closing handles before reaping fixed it. The overload regression
and eight repeated backlogged-output cancellation cycles now complete, with a
separate completion deadline so a regression does not hang the test harness.

Native GUI/GPU timing, actual operating-system clipboard acceptance, Linux PTY
execution, multi-hour workloads, and hostile unbounded parser allocations remain
unverified. Final functional results after the shared-wait follow-up: 359 Rust unit
tests and 3 CLI integration tests passed
with the clean-shell serial command above. All 18 Python tests passed, as did
formatting, Clippy with warnings denied, the locked build, and shell syntax
checks. The full diff was reviewed, including the interaction with preserved
autoscroll edits. Shared-wait regressions additionally cover fair reservations,
exhaustion and next-frame recovery, uncontended locks after exhaustion, repeated
layout passes, nested windows, scheduler overshoot, minimized windows, active
workspace changes and maximized panes. The benchmark records retain their
measurement boundaries.
