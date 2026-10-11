//! Long-running work: exports, batch image ops, provider calls.
//!
//! A job is a shared [`JobState`] plus a [`Handle`] handed to the closure that does
//! the work. Cancellation and progress are *cooperative*: the closure polls
//! [`Handle::cancelled`] and reports [`Handle::progress`]. Nothing in here can
//! preempt a thread that is stuck inside a syscall — that is stated plainly rather
//! than promised, because a promise would be a lie.
//!
//! The engine has no async runtime, so [`run`] executes the closure on the **current
//! thread** and arms a watchdog for its time budget. If the budget expires the
//! watchdog marks the job `JOB_TIMEOUT`, raises the cancel flag so cooperative work
//! inside stops at its next poll, and releases its own thread; [`run`] then returns
//! the timeout error. The calling thread stays inside the closure until the closure
//! itself returns — the job is reported as timed out honestly, and a wedged syscall
//! is still wedged (the timeout is recorded, the process is not rescued).

use crate::error::{Fail, R};
use crate::session::JOB_LOG_CAP;
use crate::util;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

/// Time budget of [`run`] when `AGENTCANVAS_JOB_TIMEOUT_SECS` is unset.
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// `snapshot()` reports at most this many jobs.
pub const SNAPSHOT_CAP: usize = 50;
/// How often the watchdog re-checks the clock.
const POLL: Duration = Duration::from_millis(100);

/// `AGENTCANVAS_JOB_TIMEOUT_SECS`, falling back to [`DEFAULT_TIMEOUT_SECS`].
pub fn timeout_secs() -> u64 {
    match std::env::var("AGENTCANVAS_JOB_TIMEOUT_SECS") {
        Ok(s) => match s.trim().parse::<u64>() {
            Ok(0) => u64::MAX, // 0 documented as "no budget"
            Ok(n) if n > 0 => n,
            _ => DEFAULT_TIMEOUT_SECS,
        },
        Err(_) => DEFAULT_TIMEOUT_SECS,
    }
}

/// The shared state behind one job. Cheap to clone (everything is `Arc`).
pub struct JobState {
    /// `job_<8>`
    pub id: String,
    /// Free-form kind, e.g. `export`, `image.batch`, `task`.
    pub kind: String,
    /// Human label shown in `render.jobs`.
    pub label: String,
    started_ms: u64,
    ended_ms: AtomicU64,
    /// Set by [`finish`], [`cancel`] or the watchdog: the job is terminal.
    done: AtomicBool,
    /// Claimed by whichever of `finish` / watchdog got there first.
    terminal: AtomicBool,
    /// Cooperative cancellation flag.
    cancel: AtomicBool,
    /// `ok` once terminal; `false` for a failed or cancelled job.
    ok: AtomicBool,
    /// Progress in permille (0..=1000) so it can be an atomic integer.
    progress: AtomicU32,
    stage: RwLock<String>,
    logs: RwLock<Vec<String>>,
    error: RwLock<Option<Value>>,
    result: RwLock<Option<Value>>,
}

impl JobState {
    fn new(id: String, kind: &str, label: &str) -> Arc<Self> {
        Arc::new(Self {
            id,
            kind: kind.to_string(),
            label: label.to_string(),
            started_ms: util::now_ms() as u64,
            ended_ms: AtomicU64::new(0),
            done: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            ok: AtomicBool::new(false),
            progress: AtomicU32::new(0),
            stage: RwLock::new(String::new()),
            logs: RwLock::new(Vec::new()),
            error: RwLock::new(None),
            result: RwLock::new(None),
        })
    }

    fn set_stage(&self, s: &str) {
        let mut g = self.stage.write().unwrap_or_else(|e| e.into_inner());
        *g = s.trim().to_string();
    }

    fn stage(&self) -> String {
        self.stage.read().map(|g| g.clone()).unwrap_or_default()
    }

    fn push_log(&self, msg: &str) {
        let mut g = self.logs.write().unwrap_or_else(|e| e.into_inner());
        g.push(msg.trim().to_string());
        let over = g.len().saturating_sub(JOB_LOG_CAP);
        if over > 0 {
            g.drain(0..over);
        }
    }

    fn log_lines(&self) -> Vec<String> {
        self.logs.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Claim the terminal transition. `false` when the job was already finished
    /// (so a late `finish` after a timeout cannot overwrite the honest result).
    fn claim(&self, ok: bool, err: Option<Value>) -> bool {
        if self.terminal.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.ok.store(ok, Ordering::SeqCst);
        // Progress reaches 1.0 only on success: a timed-out or failed job reports
        // how far it actually got, which is the honest number for a progress bar.
        if ok {
            self.progress.store(1000, Ordering::SeqCst);
        }
        self.ended_ms.store(util::now_ms() as u64, Ordering::SeqCst);
        if let Some(e) = err {
            if let Ok(mut g) = self.error.write() {
                *g = Some(e);
            }
        }
        self.done.store(true, Ordering::SeqCst);
        true
    }

    /// The compact `render.jobs` row.
    pub fn to_value(&self) -> Value {
        let ms = self.ended_ms.load(Ordering::SeqCst).saturating_sub(self.started_ms);
        let mut v = json!({
            "id": self.id,
            "kind": self.kind,
            "label": self.label,
            "done": self.done.load(Ordering::SeqCst),
            "stage": self.stage(),
            "ms": ms,
            "ok": self.ok.load(Ordering::SeqCst),
            "progress": self.progress_f64(),
            "cancelled": self.cancel.load(Ordering::SeqCst),
            "started": self.started_ms,
        });
        if let Some(e) = self.error.read().ok().and_then(|g| g.clone()) {
            if let Some(c) = e.get("code") {
                v["code"] = c.clone();
            }
            v["e"] = e.get("e").cloned().unwrap_or(e);
        }
        if let Some(r) = self.result.read().ok().and_then(|g| g.clone()) {
            v["result"] = r;
        }
        v
    }

    /// Progress as `0.0 ..= 1.0` (permille internally, so it is atomic).
    pub fn progress_f64(&self) -> f64 {
        crate::util::round(self.progress.load(Ordering::SeqCst) as f64 / 1000.0, 3)
    }

    /// Raw permille.
    pub fn permille(&self) -> u32 {
        self.progress.load(Ordering::SeqCst).min(1000)
    }

    fn is_live(&self) -> bool {
        !self.done.load(Ordering::SeqCst)
    }
}

/// Handed to the work closure: report progress, poll cancellation, log.
#[derive(Clone)]
pub struct Handle {
    st: Arc<JobState>,
}

impl Handle {
    /// The `job_<8>` id this handle drives.
    pub fn id(&self) -> &str {
        &self.st.id
    }

    /// Report progress. `done` is normalised to `0..=1` (values in `0..=1000` are
    /// taken as permille, which is what a caller counting items usually has handy);
    /// a non-finite value leaves the previous progress alone.
    pub fn progress(&self, done: f64, stage: &str) {
        if done.is_finite() {
            let permille = if done <= 1.0 {
                (done.clamp(0.0, 1.0) * 1000.0).round() as u32
            } else if done <= 1000.0 {
                done.round().clamp(0.0, 1000.0) as u32
            } else {
                1000
            };
            // Never go backwards: a rerun of a stage should not look like a reset.
            self.st.progress.fetch_max(permille, Ordering::SeqCst);
        }
        // The stage name is reported either way: a caller that cannot say how far
        // it got can still say *where* it is.
        self.st.set_stage(stage);
    }

    /// Did someone ask this job to stop? Poll it inside every loop iteration.
    ///
    /// Only the cancel flag answers this — the watchdog sets it when a budget
    /// expires, so cooperative work still stops. A job that finished *successfully*
    /// is terminal without having been cancelled, and `done()` reports that.
    pub fn cancelled(&self) -> bool {
        self.st.cancel.load(Ordering::SeqCst)
    }

    /// Append one line to the bounded job log (oldest dropped past
    /// [`JOB_LOG_CAP`]). Kept in memory only.
    pub fn log(&self, msg: &str) {
        self.st.push_log(msg);
    }

    /// `true` once the job is terminal (finished, failed, cancelled or timed out).
    pub fn done(&self) -> bool {
        !self.st.is_live()
    }

    /// Terminalise early with an error, e.g. a provider call that gave up.
    /// Returns `false` when the job was already terminal.
    pub fn fail(&self, code: &str, msg: &str) -> bool {
        self.st.claim(false, Some(json!({"code": code, "e": msg})))
    }

    /// Terminalise successfully. Returns `false` when already terminal.
    pub fn succeed(&self, result: Value) -> bool {
        if self.st.claim(true, None) {
            if let Ok(mut g) = self.st.result.write() {
                *g = Some(result);
            }
            true
        } else {
            false
        }
    }

    /// Perceptual progress for a UI bar.
    pub fn stage(&self) -> String {
        self.st.stage()
    }
}

type Table = HashMap<String, (u64, Arc<JobState>)>;
static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(1);
/// Jobs remembered before the oldest finished one is evicted.
const TABLE_CAP: usize = 500;

fn table() -> &'static Mutex<Table> {
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn locked() -> std::sync::MutexGuard<'static, Table> {
    table().lock().unwrap_or_else(|e| e.into_inner())
}

fn get(id: &str) -> Option<Arc<JobState>> {
    locked().get(id).map(|(_, s)| s.clone())
}

/// Register a job and return its id (`job_<8>`).
///
/// The job starts at 0 % and stays live until [`finish`] or the watchdog ends it, so
/// a client that polls `render.jobs` right after starting already sees it.
pub fn start(kind: &str, label: &str) -> String {
    let st_for = |id: String| JobState::new(id, kind, label);
    let mut id = job_id(label);
    loop {
        let mut g = locked();
        if !g.contains_key(&id) {
            let st = st_for(id.clone());
            g.insert(id.clone(), (SEQ.fetch_add(1, Ordering::SeqCst), st));
            break;
        }
        id = job_id(&format!("{label}/{}", g.len()));
    }
    // Eviction happens after inserting so the ordering counter stays monotonic.
    let mut g = locked();
    if g.len() > TABLE_CAP {
        let mut dead: Vec<(u64, String)> = g
            .iter()
            .filter(|(_, s)| !s.1.is_live())
            .map(|(k, (seq, _))| (*seq, k.clone()))
            .collect();
        dead.sort_unstable();
        let cut = g.len() - TABLE_CAP;
        for (_, k) in dead.into_iter().take(cut) {
            g.remove(&k);
        }
    }
    id
}

/// `job_` + 8 hex characters, derived from a unique engine id plus the label.
fn job_id(label: &str) -> String {
    let seed = format!("{}|{}|{}", util::id("job"), label, util::now_ms());
    let h = util::sha256_bytes(seed.as_bytes());
    format!("job_{}", &h[..8])
}

/// Report the outcome of a job. Idempotent: the first terminal call wins, so a
/// timeout recorded by the watchdog is never overwritten by a late `finish`.
pub fn finish(id: &str, ok: bool, result: Value) {
    if let Some(st) = get(id) {
        let err = if ok {
            None
        } else if result.is_object() {
            Some(result.clone())
        } else {
            Some(json!({"code": "INTERNAL", "e": result}))
        };
        if st.claim(ok, err) && ok {
            if let Ok(mut g) = st.result.write() {
                *g = Some(result);
            }
        }
    }
}

/// [`finish`] that reports whether *this* call was the terminal transition
/// (`false` when the job was unknown or already finished).
pub fn finish_once(id: &str, ok: bool, result: Value) -> bool {
    match get(id) {
        Some(st) => {
            let was_live = st.is_live();
            finish(id, ok, result);
            was_live
        }
        None => false,
    }
}

/// Ask a job to stop. `true` when the job existed *and* was still live — cancelling
/// a finished job is a no-op and reports `false`, as the contract requires.
pub fn cancel(id: &str) -> bool {
    match get(id) {
        Some(st) if st.is_live() => {
            st.cancel.store(true, Ordering::SeqCst);
            let tag = format!("cancel requested {}", util::stamp());
            st.push_log(&tag);
            true
        }
        _ => false,
    }
}

/// One job as JSON, including its bounded log.
pub fn get_job(id: &str) -> Option<Value> {
    get(id).map(|s| {
        let mut v = s.to_value();
        v["logs"] = json!(s.log_lines());
        v
    })
}

/// `true` while the job exists and has not reached a terminal state.
pub fn is_live(id: &str) -> bool {
    get(id).map(|s| s.is_live()).unwrap_or(false)
}

/// The last [`SNAPSHOT_CAP`] jobs, oldest first, as
/// `{id, kind, label, done, stage, ms, ok, e?, progress, cancelled}`.
pub fn snapshot() -> Vec<Value> {
    let mut rows: Vec<(u64, Arc<JobState>)> = locked().values().cloned().collect();
    rows.sort_by_key(|(seq, _)| *seq);
    let skip = rows.len().saturating_sub(SNAPSHOT_CAP);
    rows.into_iter().skip(skip).map(|(_, s)| s.to_value()).collect()
}

/// Forget every finished job (`clear`). Live jobs stay: without thread cancellation
/// the honest options are "keep reporting it" or "lie about it", so they are kept.
/// Returns the number of jobs dropped.
pub fn clear() -> usize {
    let mut g = locked();
    let before = g.len();
    g.retain(|_, (_, s)| s.is_live());
    before - g.len()
}

/// How many jobs are registered / still live.
pub fn counts() -> (usize, usize) {
    let g = locked();
    (g.len(), g.values().filter(|(_, s)| s.is_live()).count())
}

/// Run `f` on the current thread under a watchdog with the default budget.
pub fn run<T: Send + 'static>(label: &str, f: impl FnOnce(&Handle) -> R<Value> + Send + 'static) -> R<Value> {
    let budget = timeout_secs();
    // The job output is a `Value` in every case; `T` is only carried for the
    // contract signature, so it is pinned here rather than left to the caller.
    run_with_budget::<Value>(label, budget, f)
}

/// [`run`] with an explicit budget in seconds (used by tests and by callers that
/// know a job is long). `0` means no watchdog.
pub fn run_with_budget<T: Send + 'static>(
    label: &str,
    budget_secs: u64,
    f: impl FnOnce(&Handle) -> R<Value> + Send + 'static,
) -> R<Value> {
    let id = start("task", label);
    let st = match get(&id) {
        Some(s) => s,
        None => return Err(Fail::new("INTERNAL", "job table lost its entry")),
    };
    let handle = Handle { st: st.clone() };
    let deadline = if budget_secs > 0 {
        Instant::now() + Duration::from_secs(budget_secs)
    } else {
        Instant::now() + Duration::from_secs(u64::MAX / 2)
    };

    // `thread::scope` joins the watchdog before returning, so no thread outlives
    // the call and no `'static` borrow gymnastics are needed.
    std::thread::scope(|s| {
        if budget_secs > 0 {
            let watch = st.clone();
            let label = label.to_string();
            s.spawn(move || watchdog(watch, deadline, budget_secs, &label));
        }
        // A panic inside `f` must not leave a job forever live, and must not escape
        // as a raw panic payload: it becomes an honest INTERNAL failure.
        let mut slot = Some(f);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> R<Value> {
            let g = slot.take().ok_or_else(|| Fail::new("INTERNAL", "job closure already consumed"))?;
            g(&handle)
        }));
        let out = match caught {
            Ok(r) => r,
            Err(p) => {
                let msg = panic_message(&p);
                st.claim(false, Some(json!({"code": "INTERNAL", "e": msg.clone()})));
                Err(Fail::new("INTERNAL", format!("job panicked: {msg}")))
            }
        };
        match out {
            Ok(v) => {
                if st.claim(true, None) {
                    if let Ok(mut g) = st.result.write() {
                        *g = Some(v.clone());
                    }
                    Ok(v)
                } else {
                    // The watchdog got there first: report the timeout, not the
                    // late success.
                    Err(timeout_fail(&st, &id, budget_secs))
                }
            }
            Err(e) => {
                if st.claim(false, Some(e.to_value())) {
                    Err(e)
                } else {
                    Err(timeout_fail(&st, &id, budget_secs))
                }
            }
        }
    })
}

/// Best-effort panic text, without needing `P: RefUnwindSafe`.
fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn timeout_fail(st: &Arc<JobState>, id: &str, budget: u64) -> Fail {
    let msg = match st.error.read().ok().and_then(|g| g.clone()) {
        Some(e) => e.get("e").and_then(|v| v.as_str()).unwrap_or("job exceeded its time budget").to_string(),
        None => format!("job \"{id}\" exceeded its time budget"),
    };
    Fail::new("JOB_TIMEOUT", format!("{msg} (budget {budget}s)"))
        .hint("raise AGENTCANVAS_JOB_TIMEOUT_SECS, or split the work into smaller jobs")
        .details(json!({"id": id, "budgetSecs": budget, "progress": st.progress_f64()}))
        .retry()
}

fn watchdog(st: Arc<JobState>, deadline: Instant, budget: u64, label: &str) {
    loop {
        if !st.is_live() {
            return; // finished first
        }
        let now = Instant::now();
        if now >= deadline {
            if st.claim(false, Some(json!({"code": "JOB_TIMEOUT", "e": format!("\"{label}\" exceeded its time budget")}))) {
                // Signal cooperative work inside to stop at its next poll.
                st.cancel.store(true, Ordering::SeqCst);
                st.push_log(&format!("timeout after {budget}s"));
            }
            return;
        }
        let remain = deadline - now;
        std::thread::sleep(remain.min(POLL));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_monotonic_and_clamped() {
        let id = start("test", "progress");
        let h = Handle { st: get(&id).unwrap() };
        h.progress(0.25, "a quarter");
        assert_eq!(h.st.permille(), 250);
        assert_eq!(h.st.progress_f64(), 0.25);
        h.progress(900.0, "given as permille");
        assert_eq!(h.st.permille(), 900);
        h.progress(0.1, "backwards is ignored");
        assert_eq!(h.st.permille(), 900);
        h.progress(f64::NAN, "nan is ignored");
        assert_eq!(h.st.permille(), 900);
        assert_eq!(h.stage(), "nan is ignored");
        finish(&id, true, json!({"ok": true}));
        assert_eq!(h.st.progress_f64(), 1.0);
        assert!(h.done());
        assert!(!h.cancelled());
    }

    #[test]
    fn a_failed_job_keeps_its_real_progress() {
        let id = start("test", "failed");
        let h = Handle { st: get(&id).unwrap() };
        h.progress(0.4, "middle");
        finish(&id, false, json!({"code": "RENDER_FAILED", "e": "boom"}));
        assert_eq!(h.st.progress_f64(), 0.4);
        let v = get_job(&id).unwrap();
        assert_eq!(v["code"], "RENDER_FAILED");
        assert_eq!(v["e"], "boom");
        assert_eq!(v["ok"], false);
    }

    #[test]
    fn cancel_finished_job_is_false() {
        let id = start("test", "done-job");
        finish(&id, true, json!(null));
        assert!(!cancel(&id));
        assert!(!cancel("job_nosuchid"));
        assert!(!finish_once(&id, true, json!(null)));
        let live = start("test", "live-job");
        assert!(cancel(&live));
        assert!(get(&live).unwrap().cancel.load(Ordering::SeqCst));
        assert!(is_live(&live));
    }

    #[test]
    fn ids_are_job_prefix_and_unique() {
        let a = start("test", "x");
        let b = start("test", "x");
        assert!(a.starts_with("job_") && a.len() == 12, "bad id {a}");
        assert_ne!(a, b);
    }

    #[test]
    fn run_reports_timeout_and_stops_cooperative_work() {
        let r = run_with_budget::<Value>("slow", 1, |h| {
            for _ in 0..100 {
                if h.cancelled() {
                    return Err(Fail::new("CANCELLED", "stopped after timeout"));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(json!({"finished": true}))
        });
        assert!(r.is_err(), "expected a timeout error, got {r:?}");
        assert_eq!(r.unwrap_err().code, "JOB_TIMEOUT");
    }

    #[test]
    fn run_finishes_successfully() {
        let v = run::<Value>("quick", |_| Ok(json!({"n": 1}))).unwrap();
        assert_eq!(v["n"], 1);
    }

    #[test]
    fn run_records_a_failed_job() {
        let e = run::<Value>("failing", |_| Err(Fail::new("NOT_FOUND", "no such file"))).unwrap_err();
        assert_eq!(e.code, "NOT_FOUND");
        // Rows this test owns, so the assertions cannot race other test threads.
        let id = start("test", "row-check");
        finish(&id, false, json!({"code": "IO", "e": "disk"}));
        let row = get_job(&id).expect("row");
        assert_eq!(row["done"], true);
        assert_eq!(row["ok"], false);
        assert_eq!(row["code"], "IO");
        assert_eq!(row["progress"], 0.0, "a failed job does not claim to be done");
        let snap = snapshot();
        assert!(snap.iter().any(|r| r["id"] == id), "snapshot missed the job");
        assert!(snap.len() <= SNAPSHOT_CAP);
        let ok = start("test", "row-ok");
        finish(&ok, true, json!({"files": 2}));
        assert_eq!(get_job(&ok).unwrap()["progress"], 1.0);
    }
}
