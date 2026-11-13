use std::{
    collections::{HashMap, HashSet},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use crate::connection::{Connection, DbKind};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 10 });
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const KEEPALIVE_RETRIES: u32 = 3;
const CANCEL_GRACE: Duration = Duration::from_millis(400);
const CANCEL_RETRY: Duration = Duration::from_millis(50);
const CANCEL_ATTEMPTS: usize = 40;
const SPARE_RETRIES: u32 = 2;
const SQLITE_PROGRESS_OPS: i32 = 1000;
pub const MAX_CELL_CHARS: usize = 500;
pub const MAX_FULL_BYTES: usize = 64 * 1024;
const BYTES_PREVIEW: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub name: String,
    pub kind: TableKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    pub name: String,
    pub definition: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableDdl {
    pub sql: String,
    pub columns: Vec<ColumnDef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullValue {
    pub text: String,
    pub truncated: bool,
}

pub type FullValues = HashMap<(usize, usize), FullValue>;

pub type OpaqueCells = HashSet<(usize, usize)>;

#[derive(Debug, Default)]
struct Field {
    shown: Option<String>,
    full: Option<FullValue>,
    opaque: bool,
}

impl Field {
    fn shown(text: String) -> Self {
        Self {
            shown: Some(text),
            full: None,
            opaque: false,
        }
    }

    fn opaque(text: String) -> Self {
        Self {
            shown: Some(text),
            full: None,
            opaque: true,
        }
    }

    fn text(text: &str) -> Self {
        let shown = render_text(text);
        let full = (shown != text).then(|| {
            let end = text.floor_char_boundary(MAX_FULL_BYTES);
            FullValue {
                text: text[..end].to_string(),
                truncated: end < text.len(),
            }
        });
        Self {
            shown: Some(shown),
            full,
            opaque: false,
        }
    }
}

fn split_fields(fields: Vec<Field>, row: usize, full: &mut FullValues) -> Vec<Option<String>> {
    fields
        .into_iter()
        .enumerate()
        .map(|(column, field)| {
            if let Some(value) = field.full {
                full.insert((row, column), value);
            }
            field.shown
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub schema: String,
    pub table: String,
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Option<String>>>,
    pub full: FullValues,
    pub opaque: OpaqueCells,
    pub primary_key: Vec<String>,
    pub offset: u64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowUpdate {
    pub schema: String,
    pub table: String,
    pub key: Vec<(String, String)>,
    pub values: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptResult {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Option<String>>>,
    pub full: FullValues,
    pub truncated: bool,
    pub statements: usize,
    pub result_sets: usize,
    pub affected: u64,
    pub elapsed: Duration,
}

impl ScriptResult {
    fn begin_set(&mut self, columns: Vec<ColumnInfo>) {
        self.result_sets += 1;
        self.columns = columns;
        self.rows.clear();
        self.full.clear();
        self.truncated = false;
    }

    #[cfg(test)]
    fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }

    fn is_full(&self, limit: u32) -> bool {
        self.rows.len() >= limit as usize
    }

    fn push(&mut self, limit: u32, row: impl FnOnce() -> Vec<Field>) {
        if self.is_full(limit) {
            self.truncated = true;
        } else {
            let row = split_fields(row(), self.rows.len(), &mut self.full);
            self.rows.push(row);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    ListSchemas {
        id: u64,
    },
    ListTables {
        id: u64,
        schema: String,
    },
    FetchPage {
        id: u64,
        schema: String,
        table: String,
        offset: u64,
        limit: u32,
    },
    CountRows {
        id: u64,
        schema: String,
        table: String,
    },
    FetchDdl {
        id: u64,
        schema: String,
        table: String,
    },
    RunScript {
        id: u64,
        sql: String,
        limit: u32,
    },
    UpdateRow {
        id: u64,
        update: RowUpdate,
    },
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Request::ListSchemas { id }
            | Request::ListTables { id, .. }
            | Request::FetchPage { id, .. }
            | Request::CountRows { id, .. }
            | Request::FetchDdl { id, .. }
            | Request::RunScript { id, .. }
            | Request::UpdateRow { id, .. } => *id,
        }
    }

    fn has_side_effects(&self) -> bool {
        matches!(self, Request::RunScript { .. } | Request::UpdateRow { .. })
    }

    fn lost_note(&self) -> &'static str {
        match self {
            Request::UpdateRow { .. } => {
                "the connection was lost, so the change may not have been saved"
            }
            _ => "the connection was lost, so the script may have run only in part",
        }
    }

    fn replaces(&self, running: &Request) -> bool {
        matches!(
            (self, running),
            (Request::FetchPage { .. }, Request::FetchPage { .. })
                | (Request::CountRows { .. }, Request::CountRows { .. })
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Connected {
        server_info: String,
    },
    Schemas {
        id: u64,
        schemas: Vec<String>,
    },
    Tables {
        id: u64,
        schema: String,
        tables: Vec<TableInfo>,
    },
    Page {
        id: u64,
        page: Page,
    },
    Count {
        id: u64,
        count: u64,
    },
    Ddl {
        id: u64,
        ddl: TableDdl,
    },
    Script {
        id: u64,
        result: ScriptResult,
    },
    RowUpdated {
        id: u64,
    },
    Error {
        id: Option<u64>,
        message: String,
    },
}

#[cfg(test)]
impl Response {
    pub fn id(&self) -> Option<u64> {
        match self {
            Response::Connected { .. } => None,
            Response::Schemas { id, .. }
            | Response::Tables { id, .. }
            | Response::Page { id, .. }
            | Response::Count { id, .. }
            | Response::Ddl { id, .. }
            | Response::Script { id, .. }
            | Response::RowUpdated { id } => Some(*id),
            Response::Error { id, .. } => *id,
        }
    }
}

type Cancel = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug)]
struct Failure {
    message: String,
    cancelled: bool,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            cancelled: false,
        }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

#[derive(Default)]
struct State {
    stopped: bool,
    running: Option<Request>,
    aborted: bool,
    revoked: Option<u64>,
}

pub type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    halt: Arc<AtomicBool>,
    cancel: Mutex<Option<Cancel>>,
    cancels: AtomicU64,
    wake: Mutex<Option<Wake>>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn canceller(&self) -> Option<Cancel> {
        self.cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn begin(&self, request: &Request) -> bool {
        let mut state = self.state();
        if state.stopped {
            return false;
        }
        let revoked = state.revoked.take_if(|id| *id == request.id()).is_some();
        state.running = Some(request.clone());
        state.aborted = revoked;
        self.halt.store(revoked, Ordering::SeqCst);
        true
    }

    fn finish(&self) -> (bool, bool) {
        let mut state = self.state();
        state.running = None;
        (state.stopped, state.aborted)
    }

    fn halted(&self) -> bool {
        let state = self.state();
        state.stopped || state.aborted
    }

    fn set_wake(&self, wake: Option<Wake>) {
        *self.wake.lock().unwrap_or_else(PoisonError::into_inner) = wake;
    }

    fn wake(&self) {
        let wake = self
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    fn is_running(&self, id: Option<u64>) -> bool {
        self.state()
            .running
            .as_ref()
            .is_some_and(|running| id.is_none_or(|id| running.id() == id))
    }
}

fn chase(shared: &Arc<Shared>, id: Option<u64>, first: Option<Sender<()>>) {
    if shared.canceller().is_none() {
        return;
    }
    let shared = Arc::clone(shared);
    thread::spawn(move || {
        let mut first = first;
        let mut pause = CANCEL_RETRY;
        for _ in 0..CANCEL_ATTEMPTS {
            if !shared.is_running(id) {
                break;
            }
            let Some(cancel) = shared.canceller() else {
                break;
            };
            shared.cancels.fetch_add(1, Ordering::SeqCst);
            cancel();
            shared.cancels.fetch_add(1, Ordering::SeqCst);
            first.take();
            thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_secs(1));
        }
    });
}

struct Lane {
    requests: Sender<Request>,
    shared: Arc<Shared>,
}

impl Lane {
    fn spawn(
        conn: Connection,
        password: Option<String>,
        responses: Sender<Response>,
        announce: bool,
        writable: bool,
    ) -> Lane {
        let (requests, request_rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let worker = Worker {
            conn,
            password,
            requests: request_rx,
            responses,
            shared: Arc::clone(&shared),
            announce,
            writable,
        };
        thread::spawn(move || worker.run());
        Lane { requests, shared }
    }

    fn send(&self, request: Request) {
        let superseded = {
            let mut state = self.shared.state();
            match &state.running {
                Some(running) if !state.aborted && request.replaces(running) => {
                    let id = running.id();
                    state.aborted = true;
                    self.shared.halt.store(true, Ordering::SeqCst);
                    Some(id)
                }
                _ => None,
            }
        };
        if let Some(id) = superseded {
            chase(&self.shared, Some(id), None);
        }
        let _ = self.requests.send(request);
    }

    fn cancel(&self, id: u64) {
        let running = {
            let mut state = self.shared.state();
            match &state.running {
                Some(running) if running.id() == id => {
                    let first = !state.aborted;
                    state.aborted = true;
                    self.shared.halt.store(true, Ordering::SeqCst);
                    first
                }
                _ => {
                    state.revoked = Some(id);
                    false
                }
            }
        };
        if running {
            chase(&self.shared, Some(id), None);
        }
    }

    fn stop(&self) -> Option<Receiver<()>> {
        let running = {
            let mut state = self.shared.state();
            state.stopped = true;
            self.shared.halt.store(true, Ordering::SeqCst);
            state.running.is_some()
        };
        if !running {
            return None;
        }
        let (first, sent) = mpsc::channel();
        chase(&self.shared, None, Some(first));
        Some(sent)
    }
}

pub struct Session {
    kind: DbKind,
    main: Lane,
    counts: Lane,
    scripts: Lane,
    edits: Lane,
    responses: Receiver<Response>,
    next_id: AtomicU64,
    background_drop: AtomicBool,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Session {
    pub fn spawn(conn: Connection, password: Option<String>) -> Session {
        let (response_tx, responses) = mpsc::channel();
        let lane = |announce, writable| {
            Lane::spawn(
                conn.clone(),
                password.clone(),
                response_tx.clone(),
                announce,
                writable,
            )
        };
        Session {
            kind: conn.kind,
            main: lane(true, false),
            counts: lane(false, false),
            scripts: lane(false, true),
            edits: lane(false, true),
            responses,
            next_id: AtomicU64::new(1),
            background_drop: AtomicBool::new(false),
        }
    }

    pub fn kind(&self) -> DbKind {
        self.kind
    }

    pub fn send(&self, request: Request) {
        match request {
            Request::CountRows { .. } => self.counts.send(request),
            Request::RunScript { .. } => self.scripts.send(request),
            Request::UpdateRow { .. } => self.edits.send(request),
            _ => self.main.send(request),
        }
    }

    pub fn try_recv(&self) -> Option<Response> {
        self.responses.try_recv().ok()
    }

    pub fn set_waker(&self, wake: impl Fn() + Send + Sync + 'static) {
        let wake: Wake = Arc::new(wake);
        for lane in [&self.main, &self.counts, &self.scripts, &self.edits] {
            lane.shared.set_wake(Some(Arc::clone(&wake)));
        }
    }

    pub fn drop_in_background(&self) {
        self.background_drop.store(true, Ordering::SeqCst);
    }

    pub fn drop_in_foreground(&self) {
        self.background_drop.store(false, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub fn drops_in_background(&self) -> bool {
        self.background_drop.load(Ordering::SeqCst)
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn list_schemas(&self) -> u64 {
        let id = self.next_id();
        self.send(Request::ListSchemas { id });
        id
    }

    pub fn list_tables(&self, schema: &str) -> u64 {
        let id = self.next_id();
        self.send(Request::ListTables {
            id,
            schema: schema.to_string(),
        });
        id
    }

    pub fn fetch_page(&self, schema: &str, table: &str, offset: u64, limit: u32) -> u64 {
        let id = self.next_id();
        self.send(Request::FetchPage {
            id,
            schema: schema.to_string(),
            table: table.to_string(),
            offset,
            limit,
        });
        id
    }

    pub fn count_rows(&self, schema: &str, table: &str) -> u64 {
        let id = self.next_id();
        self.send(Request::CountRows {
            id,
            schema: schema.to_string(),
            table: table.to_string(),
        });
        id
    }

    pub fn fetch_ddl(&self, schema: &str, table: &str) -> u64 {
        let id = self.next_id();
        self.send(Request::FetchDdl {
            id,
            schema: schema.to_string(),
            table: table.to_string(),
        });
        id
    }

    pub fn run_script(&self, sql: &str, limit: u32) -> u64 {
        let id = self.next_id();
        self.send(Request::RunScript {
            id,
            sql: sql.to_string(),
            limit,
        });
        id
    }

    pub fn cancel_script(&self, id: u64) {
        self.scripts.cancel(id);
    }

    pub fn update_row(&self, update: RowUpdate) -> u64 {
        let id = self.next_id();
        self.send(Request::UpdateRow { id, update });
        id
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let cancels: Vec<Receiver<()>> = [&self.main, &self.counts, &self.scripts, &self.edits]
            .into_iter()
            .filter_map(Lane::stop)
            .collect();
        if cancels.is_empty() {
            return;
        }
        let background = self.background_drop.load(Ordering::SeqCst);
        await_cancels(cancels, CANCEL_GRACE, background);
    }
}

fn await_cancels(cancels: Vec<Receiver<()>>, grace: Duration, background: bool) {
    let wait = move || {
        let deadline = Instant::now() + grace;
        for sent in cancels {
            let _ = sent.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        }
    };
    if background {
        thread::spawn(wait);
    } else {
        wait();
    }
}

struct Worker {
    conn: Connection,
    password: Option<String>,
    requests: Receiver<Request>,
    responses: Sender<Response>,
    shared: Arc<Shared>,
    announce: bool,
    writable: bool,
}

impl Worker {
    fn run(self) {
        let mut backend = None;
        if self.announce {
            match self.connect() {
                Ok(mut connected) => {
                    let server_info = connected.server_info();
                    if !self.respond(Response::Connected { server_info }) {
                        return;
                    }
                    backend = Some(connected);
                }
                Err(message) => {
                    self.respond(Response::Error { id: None, message });
                    return;
                }
            }
        }
        while let Ok(first) = self.requests.recv() {
            let mut batch = vec![first];
            batch.extend(self.requests.try_iter());
            for request in coalesce(batch) {
                if !self.shared.begin(&request) {
                    return;
                }
                let response = if self.shared.halted() {
                    Response::Error {
                        id: Some(request.id()),
                        message: "cancelled before it started".to_string(),
                    }
                } else {
                    self.serve(&mut backend, &request)
                };
                let (stopped, aborted) = self.shared.finish();
                if stopped {
                    return;
                }
                let wanted = !aborted || request.has_side_effects();
                if wanted && !self.respond(response) {
                    return;
                }
            }
        }
    }

    fn respond(&self, response: Response) -> bool {
        let sent = self.responses.send(response).is_ok();
        if sent {
            self.shared.wake();
        }
        sent
    }

    fn connect(&self) -> Result<Box<dyn Backend>, String> {
        let conn = self.conn.clone();
        let password = self.password.clone();
        let halt = Arc::clone(&self.shared.halt);
        let writable = self.writable;
        let (result, connected) = mpsc::channel();
        thread::spawn(move || {
            let _ = result.send(open(&conn, password.as_deref(), halt, writable));
        });
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let backend = loop {
            match connected.recv_timeout(Duration::from_millis(50)) {
                Ok(backend) => break backend?,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("internal error while connecting".into());
                }
                Err(RecvTimeoutError::Timeout) if self.shared.halt.load(Ordering::SeqCst) => {
                    return Err("connection cancelled".into());
                }
                Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    return Err(format!(
                        "timed out after {}s waiting for the server to respond",
                        CONNECT_TIMEOUT.as_secs()
                    ));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        };
        *self
            .shared
            .cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = backend.canceller();
        Ok(backend)
    }

    fn serve(&self, backend: &mut Option<Box<dyn Backend>>, request: &Request) -> Response {
        let mut spare = SPARE_RETRIES;
        loop {
            let cancels = self.shared.cancels.load(Ordering::SeqCst);
            let failure = match self.attempt(backend, request) {
                Ok(response) => return response,
                Err(failure) => failure,
            };
            if failure.cancelled && !self.shared.halted() && !request.has_side_effects() {
                if self.shared.cancels.load(Ordering::SeqCst) != cancels {
                    continue;
                }
                if spare > 0 {
                    spare -= 1;
                    continue;
                }
            }
            return Response::Error {
                id: Some(request.id()),
                message: failure.message,
            };
        }
    }

    fn attempt(
        &self,
        slot: &mut Option<Box<dyn Backend>>,
        request: &Request,
    ) -> Result<Response, Failure> {
        if request.has_side_effects() && slot.as_mut().is_some_and(|backend| !backend.is_alive()) {
            *slot = None;
        }
        let backend = match slot {
            Some(backend) => backend,
            None => slot.insert(self.connect()?),
        };
        match run(backend.as_mut(), request) {
            Err(failure) if !failure.cancelled && !self.shared.halted() && backend.is_broken() => {
                if request.has_side_effects() {
                    *slot = None;
                    return Err(format!("{} ({})", failure.message, request.lost_note()).into());
                }
                let fresh = self.connect().map_err(|reconnect| {
                    format!("{} (reconnect failed: {reconnect})", failure.message)
                })?;
                run(slot.insert(fresh).as_mut(), request)
            }
            other => other,
        }
    }
}

fn open(
    conn: &Connection,
    password: Option<&str>,
    halt: Arc<AtomicBool>,
    writable: bool,
) -> Result<Box<dyn Backend>, String> {
    Ok(match conn.kind {
        DbKind::Postgres => {
            Box::new(PgBackend::connect(conn, password).map_err(|failure| failure.message)?)
        }
        DbKind::MySql => {
            Box::new(MyBackend::connect(conn, password).map_err(|failure| failure.message)?)
        }
        DbKind::Sqlite => Box::new(SqliteBackend::open(&conn.database, halt, writable)?),
    })
}

fn run(backend: &mut dyn Backend, request: &Request) -> Result<Response, Failure> {
    catch_unwind(AssertUnwindSafe(|| execute(backend, request)))
        .unwrap_or_else(|_| Err("internal error while running the query".into()))
}

fn coalesce(batch: Vec<Request>) -> Vec<Request> {
    let last_page = batch
        .iter()
        .rposition(|r| matches!(r, Request::FetchPage { .. }));
    let last_count = batch
        .iter()
        .rposition(|r| matches!(r, Request::CountRows { .. }));
    batch
        .into_iter()
        .enumerate()
        .filter(|(i, request)| match request {
            Request::FetchPage { .. } => Some(*i) == last_page,
            Request::CountRows { .. } => Some(*i) == last_count,
            _ => true,
        })
        .map(|(_, request)| request)
        .collect()
}

fn execute(backend: &mut dyn Backend, request: &Request) -> Result<Response, Failure> {
    Ok(match request {
        Request::ListSchemas { id } => Response::Schemas {
            id: *id,
            schemas: backend.schemas()?,
        },
        Request::ListTables { id, schema } => Response::Tables {
            id: *id,
            schema: schema.clone(),
            tables: backend.tables(schema)?,
        },
        Request::FetchPage {
            id,
            schema,
            table,
            offset,
            limit,
        } => {
            let TableRows {
                columns,
                rows,
                primary_key,
            } = backend.rows(schema, table, *offset, *limit)?;
            let (rows, has_more) = split_page(rows, *limit);
            let mut full = FullValues::new();
            let mut opaque = OpaqueCells::new();
            let rows = rows
                .into_iter()
                .enumerate()
                .map(|(row, fields)| {
                    for (column, field) in fields.iter().enumerate() {
                        if field.opaque {
                            opaque.insert((row, column));
                        }
                    }
                    split_fields(fields, row, &mut full)
                })
                .collect();
            Response::Page {
                id: *id,
                page: Page {
                    schema: schema.clone(),
                    table: table.clone(),
                    columns,
                    rows,
                    full,
                    opaque,
                    primary_key,
                    offset: *offset,
                    has_more,
                },
            }
        }
        Request::CountRows { id, schema, table } => Response::Count {
            id: *id,
            count: backend.count(schema, table)?,
        },
        Request::FetchDdl { id, schema, table } => Response::Ddl {
            id: *id,
            ddl: backend.ddl(schema, table)?,
        },
        Request::RunScript { id, sql, limit } => {
            let started = Instant::now();
            let mut result = backend.script(sql, *limit)?;
            result.elapsed = started.elapsed();
            Response::Script { id: *id, result }
        }
        Request::UpdateRow { id, update } => {
            backend.update_row(update)?;
            Response::RowUpdated { id: *id }
        }
    })
}

type Rows = Vec<Vec<Field>>;

struct TableRows {
    columns: Vec<ColumnInfo>,
    rows: Rows,
    primary_key: Vec<String>,
}

trait Backend: Send {
    fn server_info(&mut self) -> String;
    fn schemas(&mut self) -> Result<Vec<String>, Failure>;
    fn tables(&mut self, schema: &str) -> Result<Vec<TableInfo>, Failure>;
    fn rows(
        &mut self,
        schema: &str,
        table: &str,
        offset: u64,
        limit: u32,
    ) -> Result<TableRows, Failure>;
    fn count(&mut self, schema: &str, table: &str) -> Result<u64, Failure>;
    fn ddl(&mut self, schema: &str, table: &str) -> Result<TableDdl, Failure>;
    fn script(&mut self, sql: &str, limit: u32) -> Result<ScriptResult, Failure>;
    fn update_row(&mut self, update: &RowUpdate) -> Result<(), Failure>;
    fn is_broken(&mut self) -> bool;
    fn is_alive(&mut self) -> bool {
        !self.is_broken()
    }
    fn canceller(&self) -> Option<Cancel>;
}

pub fn quote_ident(kind: DbKind, name: &str) -> String {
    let quote = match kind {
        DbKind::Postgres | DbKind::Sqlite => '"',
        DbKind::MySql => '`',
    };
    let mut out = String::with_capacity(name.len() + 2);
    out.push(quote);
    for c in name.chars() {
        if c == quote {
            out.push(quote);
        }
        out.push(c);
    }
    out.push(quote);
    out
}

fn qualified(kind: DbKind, schema: &str, table: &str) -> Result<String, String> {
    for name in [schema, table] {
        if name.is_empty() || name.contains('\0') {
            return Err(format!("invalid identifier {name:?}"));
        }
    }
    Ok(format!(
        "{}.{}",
        quote_ident(kind, schema),
        quote_ident(kind, table)
    ))
}

fn select_sql(
    kind: DbKind,
    schema: &str,
    table: &str,
    primary_key: &[String],
    offset: u64,
    limit: u32,
) -> Result<String, String> {
    let mut sql = format!("SELECT * FROM {}", qualified(kind, schema, table)?);
    if !primary_key.is_empty() {
        let columns: Vec<String> = primary_key.iter().map(|c| quote_ident(kind, c)).collect();
        sql.push_str(" ORDER BY ");
        sql.push_str(&columns.join(", "));
    }
    sql.push_str(&format!(" LIMIT {} OFFSET {offset}", u64::from(limit) + 1));
    Ok(sql)
}

fn update_sql(
    kind: DbKind,
    update: &RowUpdate,
    mut value: impl FnMut(Option<&str>) -> String,
) -> Result<String, String> {
    if update.values.is_empty() {
        return Err("nothing to save".to_string());
    }
    if update.key.is_empty() {
        return Err("the row has no key to find it by".to_string());
    }
    let target = qualified(kind, &update.schema, &update.table)?;
    let assignments: Vec<String> = update
        .values
        .iter()
        .map(|(column, new)| format!("{} = {}", quote_ident(kind, column), value(new.as_deref())))
        .collect();
    let filter: Vec<String> = update
        .key
        .iter()
        .map(|(column, old)| format!("{} = {}", quote_ident(kind, column), value(Some(old))))
        .collect();
    Ok(format!(
        "UPDATE {target} SET {} WHERE {}",
        assignments.join(", "),
        filter.join(" AND ")
    ))
}

fn placeholder(value: Option<&str>) -> String {
    if value.is_some() { "?" } else { "NULL" }.to_string()
}

fn update_params(update: &RowUpdate) -> Vec<&str> {
    update
        .values
        .iter()
        .filter_map(|(_, value)| value.as_deref())
        .chain(update.key.iter().map(|(_, value)| value.as_str()))
        .collect()
}

fn pg_literal(text: &str) -> String {
    let quoted = text.replace('\'', "''");
    if text.contains('\\') {
        format!("E'{}'", quoted.replace('\\', "\\\\"))
    } else {
        format!("'{quoted}'")
    }
}

fn matched_one(matched: u64) -> Result<(), Failure> {
    match matched {
        1 => Ok(()),
        0 => Err(
            "the row was not found: it was changed or deleted since it was loaded \
                  (nothing was saved)"
                .into(),
        ),
        n => Err(format!("the row's key matches {n} rows (nothing was saved)").into()),
    }
}

fn my_rows_matched(info: &str) -> Option<u64> {
    info.split("Rows matched:")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn count_sql(kind: DbKind, schema: &str, table: &str) -> Result<String, String> {
    Ok(format!(
        "SELECT COUNT(*) FROM {}",
        qualified(kind, schema, table)?
    ))
}

fn column_definition(parts: &[&str]) -> String {
    let parts: Vec<&str> = parts.iter().map(|part| part.trim()).collect();
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn parenthesized(expression: &str) -> String {
    if expression.starts_with('(') && expression.ends_with(')') {
        expression.to_string()
    } else {
        format!("({expression})")
    }
}

fn join_statements(statements: impl IntoIterator<Item = String>) -> String {
    statements
        .into_iter()
        .map(|statement| format!("{};", statement.trim().trim_end_matches(';').trim_end()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn no_definition(schema: &str, table: &str) -> Failure {
    format!(
        "{}.{} has no definition to show",
        render_text(schema),
        render_text(table)
    )
    .into()
}

fn split_page<T>(mut rows: Vec<T>, limit: u32) -> (Vec<T>, bool) {
    let limit = limit as usize;
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    (rows, has_more)
}

pub fn page_count(total: u64, page_size: u32) -> u64 {
    total.div_ceil(u64::from(page_size.max(1))).max(1)
}

pub fn page_number(offset: u64, page_size: u32) -> u64 {
    offset / u64::from(page_size.max(1)) + 1
}

pub fn last_page_offset(total: u64, page_size: u32) -> u64 {
    (page_count(total, page_size) - 1) * u64::from(page_size.max(1))
}

pub fn render_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_CELL_CHARS * 4));
    let mut chars = text.chars().peekable();
    let mut count = 0;
    while let Some(c) = chars.next() {
        if count == MAX_CELL_CHARS {
            out.push('…');
            break;
        }
        out.push(match c {
            '\r' if chars.peek() == Some(&'\n') => continue,
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => '↵',
            '\t' => '→',
            c if c.is_control() => '·',
            c => c,
        });
        count += 1;
    }
    out
}

pub fn render_bytes(bytes: &[u8]) -> String {
    render_bytes_preview(&bytes[..bytes.len().min(BYTES_PREVIEW)], bytes.len())
}

fn render_bytes_preview(preview: &[u8], len: usize) -> String {
    if len == 0 {
        return "<0 bytes>".to_string();
    }
    let mut out = String::from("0x");
    for byte in preview {
        out.push_str(&format!("{byte:02x}"));
    }
    if len > preview.len() {
        out.push_str(&format!("… <{len} bytes>"));
    }
    out
}

fn render_pg_bytea(text: &str) -> String {
    let Some(hex) = text.strip_prefix("\\x") else {
        return render_text(text);
    };
    let preview: Option<Vec<u8>> = hex
        .as_bytes()
        .chunks(2)
        .take(BYTES_PREVIEW)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect();
    match preview {
        Some(preview) if hex.len() % 2 == 0 => render_bytes_preview(&preview, hex.len() / 2),
        _ => render_text(text),
    }
}

fn os_user() -> Option<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|user| !user.is_empty())
}

fn host_or_local(host: &str) -> &str {
    if host.trim().is_empty() {
        "localhost"
    } else {
        host.trim()
    }
}

struct PgBackend {
    client: tokio_postgres::Client,
    runtime: tokio::runtime::Runtime,
    tls: postgres_native_tls::MakeTlsConnector,
}

impl PgBackend {
    fn connect(conn: &Connection, password: Option<&str>) -> Result<Self, Failure> {
        let user = if conn.user.is_empty() {
            os_user().unwrap_or_else(|| "postgres".to_string())
        } else {
            conn.user.clone()
        };
        let mut config = tokio_postgres::Config::new();
        config
            .host(host_or_local(&conn.host))
            .port(conn.port.unwrap_or(5432))
            .user(&user)
            .application_name("cooper")
            .ssl_mode(tokio_postgres::config::SslMode::Prefer)
            .connect_timeout(CONNECT_TIMEOUT)
            .keepalives(true)
            .keepalives_idle(KEEPALIVE_IDLE)
            .keepalives_interval(KEEPALIVE_INTERVAL)
            .keepalives_retries(KEEPALIVE_RETRIES)
            .tcp_user_timeout(KEEPALIVE_IDLE);
        if !conn.database.is_empty() {
            config.dbname(&conn.database);
        }
        if let Some(password) = password {
            config.password(password);
        }
        let connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()
            .map_err(|err| format!("TLS setup failed: {}", error_chain(&err)))?;
        let tls = postgres_native_tls::MakeTlsConnector::new(connector);
        let runtime = pg_runtime()?;
        let (client, connection) = runtime
            .block_on(config.connect(tls.clone()))
            .map_err(pg_error)?;
        runtime.spawn(connection);
        Ok(Self {
            client,
            runtime,
            tls,
        })
    }

    fn strings(
        &mut self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<Vec<String>>, Failure> {
        let rows = self
            .runtime
            .block_on(self.client.query(sql, params))
            .map_err(pg_error)?;
        rows.iter()
            .map(|row| {
                (0..row.len())
                    .map(|i| row.try_get::<_, String>(i).map_err(pg_error))
                    .collect()
            })
            .collect()
    }

    fn primary_key(&mut self, schema: &str, table: &str) -> Result<Vec<String>, Failure> {
        let rows = self.strings(
            "SELECT a.attname::text \
             FROM pg_catalog.pg_index i \
             JOIN pg_catalog.pg_class c ON c.oid = i.indrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum = ANY(i.indkey) \
             WHERE i.indisprimary AND n.nspname = $1::text AND c.relname = $2::text \
             ORDER BY array_position(i.indkey::int2[], a.attnum)",
            &[&schema, &table],
        )?;
        Ok(rows.into_iter().flatten().collect())
    }

    fn first_column(
        &mut self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<String>, Failure> {
        Ok(self
            .strings(sql, params)?
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect())
    }

    fn table_ddl(&mut self, schema: &str, table: &str) -> Result<TableDdl, Failure> {
        let relation = self.strings(PG_RELATION, &[&schema, &table])?;
        let Some([oid, kind, persistence, name, partition]) = relation.first().map(Vec::as_slice)
        else {
            return Err(no_definition(schema, table));
        };
        let oid: u32 = oid.parse().map_err(|_| no_definition(schema, table))?;
        let (kind, name) = (kind.clone(), name.clone());
        let is_partition = partition == "true";
        let persistence = match persistence.as_str() {
            "u" => "UNLOGGED ",
            "t" => "TEMPORARY ",
            _ => "",
        };

        let rows = self.strings(PG_COLUMNS, &[&oid])?;
        let (columns, mut body): (Vec<ColumnDef>, Vec<String>) =
            rows.iter().map(|row| pg_column_definition(row)).unzip();

        let mut statements = Vec::new();
        if kind == "v" || kind == "m" {
            let query =
                self.first_column("SELECT pg_catalog.pg_get_viewdef($1::oid, true)", &[&oid])?;
            let what = if kind == "m" {
                "MATERIALIZED VIEW"
            } else {
                "VIEW"
            };
            let query = query.first().map_or("", |query| query.trim_end());
            statements.push(format!("CREATE {what} {name} AS\n{query}"));
        } else {
            body.extend(
                self.first_column(PG_CONSTRAINTS, &[&oid])?
                    .into_iter()
                    .map(|constraint| format!("CONSTRAINT {constraint}")),
            );
            let what = if kind == "f" {
                "FOREIGN TABLE"
            } else {
                "TABLE"
            };
            let mut create = format!("CREATE {persistence}{what} {name} (\n");
            let lines: Vec<String> = body.iter().map(|line| format!("    {line}")).collect();
            create.push_str(&lines.join(",\n"));
            create.push_str("\n)");
            let parents = self.first_column(PG_PARENTS, &[&oid])?;
            if is_partition {
                let bound = self.first_column(
                    "SELECT pg_catalog.pg_get_expr(relpartbound, oid) \
                     FROM pg_catalog.pg_class WHERE oid = $1::oid AND relpartbound IS NOT NULL",
                    &[&oid],
                )?;
                let bound = bound.first().map_or("", String::as_str);
                create = format!("-- a partition of {} {bound}\n{create}", parents.join(", "));
            } else if !parents.is_empty() {
                create.push_str(&format!("\nINHERITS ({})", parents.join(", ")));
            }
            if kind == "p" {
                let key =
                    self.first_column("SELECT pg_catalog.pg_get_partkeydef($1::oid)", &[&oid])?;
                if let Some(key) = key.first() {
                    create.push_str(&format!("\nPARTITION BY {key}"));
                }
            }
            if kind == "f"
                && let Some(server) = self.first_column(PG_FOREIGN_SERVER, &[&oid])?.first()
            {
                create.push_str(&format!("\nSERVER {server}"));
            }
            statements.push(create);
        }
        statements.extend(self.first_column(PG_INDEXES, &[&oid])?);
        statements.extend(self.first_column(PG_TRIGGERS, &[&oid])?);
        Ok(TableDdl {
            sql: join_statements(statements),
            columns,
        })
    }

    fn column_types(&mut self, schema: &str, table: &str) -> Result<Vec<String>, Failure> {
        let rows = self.strings(
            "SELECT pg_catalog.format_type(a.atttypid, a.atttypmod) \
             FROM pg_catalog.pg_attribute a \
             JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1::text AND c.relname = $2::text \
               AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
            &[&schema, &table],
        )?;
        Ok(rows.into_iter().flatten().collect())
    }
}

const PG_RELATION: &str = "\
    SELECT c.oid::text, c.relkind::text, c.relpersistence::text, \
           pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname), \
           coalesce(pg_catalog.to_jsonb(c)->>'relispartition', 'false') \
    FROM pg_catalog.pg_class c \
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
    WHERE n.nspname = $1::text AND c.relname = $2::text";

const PG_COLUMNS: &str = "\
    SELECT a.attname::text, pg_catalog.quote_ident(a.attname), \
           pg_catalog.format_type(a.atttypid, a.atttypmod), \
           a.attnotnull::text, \
           coalesce(pg_catalog.pg_get_expr(d.adbin, d.adrelid), ''), \
           coalesce(pg_catalog.to_jsonb(a)->>'attidentity', ''), \
           coalesce(pg_catalog.to_jsonb(a)->>'attgenerated', ''), \
           EXISTS (SELECT 1 FROM pg_catalog.pg_index i \
                   WHERE i.indrelid = a.attrelid AND i.indisprimary \
                     AND a.attnum = ANY(i.indkey))::text \
    FROM pg_catalog.pg_attribute a \
    LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
    WHERE a.attrelid = $1::oid AND a.attnum > 0 AND NOT a.attisdropped \
    ORDER BY a.attnum";

const PG_CONSTRAINTS: &str = "\
    SELECT pg_catalog.quote_ident(conname) || ' ' || pg_catalog.pg_get_constraintdef(oid, true) \
    FROM pg_catalog.pg_constraint \
    WHERE conrelid = $1::oid AND contype <> 'n' \
    ORDER BY contype <> 'p', contype <> 'u', contype <> 'f', conname";

const PG_INDEXES: &str = "\
    SELECT pg_catalog.pg_get_indexdef(i.indexrelid) \
    FROM pg_catalog.pg_index i \
    JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid \
    WHERE i.indrelid = $1::oid \
      AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k WHERE k.conindid = i.indexrelid) \
    ORDER BY c.relname";

const PG_TRIGGERS: &str = "\
    SELECT pg_catalog.pg_get_triggerdef(oid, true) FROM pg_catalog.pg_trigger \
    WHERE tgrelid = $1::oid AND NOT tgisinternal ORDER BY tgname";

const PG_PARENTS: &str = "\
    SELECT pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname) \
    FROM pg_catalog.pg_inherits i \
    JOIN pg_catalog.pg_class c ON c.oid = i.inhparent \
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
    WHERE i.inhrelid = $1::oid ORDER BY i.inhseqno";

const PG_FOREIGN_SERVER: &str = "\
    SELECT pg_catalog.quote_ident(s.srvname) FROM pg_catalog.pg_foreign_table t \
    JOIN pg_catalog.pg_foreign_server s ON s.oid = t.ftserver WHERE t.ftrelid = $1::oid";

fn pg_column_definition(row: &[String]) -> (ColumnDef, String) {
    let field = |index: usize| row.get(index).map_or("", String::as_str);
    let (expression, identity, generated) = (field(4), field(5), field(6));
    let wrapped = parenthesized(expression);
    let value = match (generated, identity) {
        ("s", _) => format!("GENERATED ALWAYS AS {wrapped} STORED"),
        ("v", _) => format!("GENERATED ALWAYS AS {wrapped}"),
        (_, "a") => "GENERATED ALWAYS AS IDENTITY".to_string(),
        (_, "d") => "GENERATED BY DEFAULT AS IDENTITY".to_string(),
        _ if expression.is_empty() => String::new(),
        _ => format!("DEFAULT {expression}"),
    };
    let not_null = if field(3) == "true" { "NOT NULL" } else { "" };
    let primary = if field(7) == "true" {
        "PRIMARY KEY"
    } else {
        ""
    };
    let body = column_definition(&[field(2), not_null, &value]);
    let column = ColumnDef {
        name: field(0).to_string(),
        definition: column_definition(&[&body, primary]),
    };
    (column, format!("{} {body}", field(1)))
}

fn pg_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("could not start the PostgreSQL client: {err}"))
}

fn pg_cells(row: &tokio_postgres::SimpleQueryRow, types: &[String]) -> Vec<Field> {
    (0..row.len())
        .map(|i| match row.try_get(i) {
            Ok(Some(text)) if types.get(i).is_some_and(|t| t == "bytea") => {
                Field::opaque(render_pg_bytea(text))
            }
            Ok(Some(text)) => Field::text(text),
            Ok(None) => Field::default(),
            Err(_) => Field::opaque("<unreadable>".to_string()),
        })
        .collect()
}

fn pg_error(err: tokio_postgres::Error) -> Failure {
    let Some(db) = err.as_db_error() else {
        let message = error_chain(&err);
        return if message.contains("password missing") {
            "the server asks for a password, but none is saved for this connection".into()
        } else {
            message.into()
        };
    };
    Failure {
        message: match db.detail() {
            Some(detail) => format!("{}: {detail}", db.message()),
            None => db.message().to_string(),
        },
        cancelled: *db.code() == tokio_postgres::error::SqlState::QUERY_CANCELED,
    }
}

fn error_chain(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !text.is_empty() && !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    message
}

impl Backend for PgBackend {
    fn server_info(&mut self) -> String {
        let version = self
            .strings("SHOW server_version", &[])
            .ok()
            .and_then(|rows| rows.into_iter().flatten().next());
        match version {
            Some(version) => format!("PostgreSQL {version}"),
            None => "PostgreSQL".to_string(),
        }
    }

    fn schemas(&mut self) -> Result<Vec<String>, Failure> {
        let rows = self.strings(
            "SELECT nspname::text FROM pg_catalog.pg_namespace \
             WHERE nspname NOT IN ('pg_catalog', 'information_schema') \
               AND nspname NOT LIKE 'pg\\_toast%' AND nspname NOT LIKE 'pg\\_temp%' \
             ORDER BY nspname <> 'public', nspname",
            &[],
        )?;
        Ok(rows.into_iter().flatten().collect())
    }

    fn tables(&mut self, schema: &str) -> Result<Vec<TableInfo>, Failure> {
        let rows = self.strings(
            "SELECT c.relname::text, c.relkind::text \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1::text AND c.relkind IN ('r', 'p', 'f', 'v', 'm') \
             ORDER BY c.relname",
            &[&schema],
        )?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let mut row = row.into_iter();
                let name = row.next()?;
                let kind = match row.next()?.as_str() {
                    "v" => TableKind::View,
                    "m" => TableKind::MaterializedView,
                    _ => TableKind::Table,
                };
                Some(TableInfo { name, kind })
            })
            .collect())
    }

    fn rows(
        &mut self,
        schema: &str,
        table: &str,
        offset: u64,
        limit: u32,
    ) -> Result<TableRows, Failure> {
        let primary_key = self.primary_key(schema, table)?;
        let mut types = self.column_types(schema, table)?;
        let sql = select_sql(DbKind::Postgres, schema, table, &primary_key, offset, limit)?;
        let mut names = Vec::new();
        let mut raw = Vec::new();
        let messages = self
            .runtime
            .block_on(self.client.simple_query(&sql))
            .map_err(pg_error)?;
        for message in messages {
            match message {
                tokio_postgres::SimpleQueryMessage::RowDescription(columns) => {
                    names = columns.iter().map(|c| c.name().to_string()).collect();
                }
                tokio_postgres::SimpleQueryMessage::Row(row) => raw.push(row),
                _ => {}
            }
        }
        if types.len() != names.len() {
            types = vec![String::new(); names.len()];
        }
        let rows = raw.iter().map(|row| pg_cells(row, &types)).collect();
        let columns = names
            .into_iter()
            .zip(types)
            .map(|(name, type_name)| ColumnInfo { name, type_name })
            .collect();
        Ok(TableRows {
            columns,
            rows,
            primary_key,
        })
    }

    fn count(&mut self, schema: &str, table: &str) -> Result<u64, Failure> {
        let sql = count_sql(DbKind::Postgres, schema, table)?;
        let messages = self
            .runtime
            .block_on(self.client.simple_query(&sql))
            .map_err(pg_error)?;
        for message in messages {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = message
                && let Ok(Some(text)) = row.try_get(0)
            {
                return text
                    .parse()
                    .map_err(|_| format!("unexpected count {text:?}").into());
            }
        }
        Err("count query returned no rows".into())
    }

    fn ddl(&mut self, schema: &str, table: &str) -> Result<TableDdl, Failure> {
        self.table_ddl(schema, table)
    }

    fn script(&mut self, sql: &str, limit: u32) -> Result<ScriptResult, Failure> {
        use futures_util::TryStreamExt;
        use tokio_postgres::SimpleQueryMessage;
        let client = &self.client;
        self.runtime.block_on(async {
            let mut out = ScriptResult::default();
            let mut in_set = false;
            let stream = client.simple_query_raw(sql).await.map_err(pg_error)?;
            let mut stream = std::pin::pin!(stream);
            while let Some(message) = stream.try_next().await.map_err(pg_error)? {
                match message {
                    SimpleQueryMessage::RowDescription(columns) => {
                        in_set = true;
                        out.begin_set(
                            columns
                                .iter()
                                .map(|column| ColumnInfo {
                                    name: column.name().to_string(),
                                    type_name: String::new(),
                                })
                                .collect(),
                        );
                    }
                    SimpleQueryMessage::Row(row) => out.push(limit, || pg_cells(&row, &[])),
                    SimpleQueryMessage::CommandComplete(rows) => {
                        out.statements += 1;
                        if !std::mem::take(&mut in_set) {
                            out.affected += rows;
                        }
                    }
                    _ => {}
                }
            }
            Ok(out)
        })
    }

    fn update_row(&mut self, update: &RowUpdate) -> Result<(), Failure> {
        use tokio_postgres::SimpleQueryMessage;
        let sql = update_sql(DbKind::Postgres, update, |value| {
            value.map_or_else(|| "NULL".to_string(), pg_literal)
        })?;
        let client = &mut self.client;
        self.runtime.block_on(async {
            let transaction = client.transaction().await.map_err(pg_error)?;
            let matched = transaction
                .simple_query(&sql)
                .await
                .map_err(pg_error)?
                .iter()
                .map(|message| match message {
                    SimpleQueryMessage::CommandComplete(rows) => *rows,
                    _ => 0,
                })
                .sum();
            matched_one(matched)?;
            transaction.commit().await.map_err(pg_error)
        })
    }

    fn is_broken(&mut self) -> bool {
        self.client.is_closed()
    }

    fn is_alive(&mut self) -> bool {
        let client = &self.client;
        self.runtime
            .block_on(async {
                tokio::time::timeout(CONNECT_TIMEOUT, client.simple_query("")).await
            })
            .is_ok_and(|pinged| pinged.is_ok())
    }

    fn canceller(&self) -> Option<Cancel> {
        let token = self.client.cancel_token();
        let tls = self.tls.clone();
        Some(Arc::new(move || {
            if let Ok(runtime) = pg_runtime() {
                let _ = runtime.block_on(token.cancel_query(tls.clone()));
            }
        }))
    }
}

const MYSQL_SYSTEM_SCHEMAS: [&str; 4] =
    ["information_schema", "mysql", "performance_schema", "sys"];

struct MyBackend {
    conn: mysql::Conn,
    opts: mysql::Opts,
    database: String,
}

impl MyBackend {
    fn connect(conn: &Connection, password: Option<&str>) -> Result<Self, Failure> {
        let user = if conn.user.is_empty() {
            os_user()
        } else {
            Some(conn.user.clone())
        };
        let opts = |tls: bool| {
            let builder = mysql::OptsBuilder::new()
                .ip_or_hostname(Some(host_or_local(&conn.host)))
                .tcp_port(conn.port.unwrap_or(3306))
                .user(user.clone())
                .pass(password)
                .db_name((!conn.database.is_empty()).then_some(conn.database.as_str()))
                .prefer_socket(false)
                .tcp_connect_timeout(Some(CONNECT_TIMEOUT))
                .tcp_keepalive_time_ms(Some(KEEPALIVE_IDLE.as_millis() as u32))
                .ssl_opts(tls.then(|| {
                    mysql::SslOpts::default()
                        .with_danger_accept_invalid_certs(true)
                        .with_danger_skip_domain_validation(true)
                }));
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            let builder = builder
                .tcp_keepalive_probe_interval_secs(Some(KEEPALIVE_INTERVAL.as_secs() as u32))
                .tcp_keepalive_probe_count(Some(KEEPALIVE_RETRIES));
            #[cfg(target_os = "linux")]
            let builder = builder.tcp_user_timeout_ms(Some(KEEPALIVE_IDLE.as_millis() as u32));
            mysql::Opts::from(builder)
        };
        let mut used = opts(true);
        let connected = match mysql::Conn::new(used.clone()) {
            Err(mysql::Error::TlsError(_))
            | Err(mysql::Error::DriverError(mysql::DriverError::TlsNotSupported)) => {
                used = opts(false);
                mysql::Conn::new(used.clone())
            }
            other => other,
        };
        Ok(Self {
            conn: connected.map_err(my_error)?,
            opts: used,
            database: conn.database.clone(),
        })
    }

    fn strings<P: Into<mysql::Params>>(
        &mut self,
        sql: &str,
        params: P,
    ) -> Result<Vec<Vec<String>>, Failure> {
        use mysql::prelude::Queryable;
        let rows: Vec<mysql::Row> = self.conn.exec(sql, params).map_err(my_error)?;
        Ok(rows
            .into_iter()
            .map(|row| row.unwrap().into_iter().map(my_plain_text).collect())
            .collect())
    }
}

const MYSQL_QUERY_INTERRUPTED: u16 = 1317;

fn my_error(err: mysql::Error) -> Failure {
    match err {
        mysql::Error::MySqlError(err) => Failure {
            message: format!("{} (error {})", err.message, err.code),
            cancelled: err.code == MYSQL_QUERY_INTERRUPTED,
        },
        other => error_chain(&other).into(),
    }
}

fn my_plain_text(value: mysql::Value) -> String {
    match value {
        mysql::Value::NULL => String::new(),
        mysql::Value::Bytes(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        mysql::Value::Int(n) => n.to_string(),
        mysql::Value::UInt(n) => n.to_string(),
        other => other.as_sql(true).trim_matches('\'').to_string(),
    }
}

fn my_is_binary(column: &mysql::Column) -> bool {
    use mysql::consts::ColumnType::*;
    const BINARY_CHARSET: u16 = 63;
    match column.column_type() {
        MYSQL_TYPE_BIT | MYSQL_TYPE_GEOMETRY => true,
        MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_BLOB
        | MYSQL_TYPE_VARCHAR
        | MYSQL_TYPE_VAR_STRING
        | MYSQL_TYPE_STRING => column.character_set() == BINARY_CHARSET,
        _ => false,
    }
}

fn my_render(value: mysql::Value, binary: bool) -> Field {
    match value {
        mysql::Value::NULL => Field::default(),
        mysql::Value::Bytes(bytes) if binary => Field::opaque(render_bytes(&bytes)),
        mysql::Value::Bytes(bytes) => match std::str::from_utf8(&bytes) {
            Ok(text) => Field::text(text),
            Err(_) => Field::opaque(render_bytes(&bytes)),
        },
        other => Field::text(&my_plain_text(other)),
    }
}

fn my_cells(row: mysql::Row, binary: &[bool]) -> Vec<Field> {
    row.unwrap()
        .into_iter()
        .enumerate()
        .map(|(i, value)| my_render(value, binary.get(i).copied().unwrap_or(false)))
        .collect()
}

const MYSQL_NUMERIC_TYPES: [&str; 11] = [
    "tinyint",
    "smallint",
    "mediumint",
    "int",
    "bigint",
    "decimal",
    "numeric",
    "float",
    "double",
    "real",
    "year",
];

fn my_default(value: &str, type_name: &str, expression: bool) -> String {
    let lower = value.to_lowercase();
    let type_name = type_name.to_lowercase();
    let number = value.parse::<f64>().is_ok()
        && MYSQL_NUMERIC_TYPES
            .iter()
            .any(|numeric| type_name.starts_with(numeric));
    let plain = expression
        || number
        || value.starts_with('\'')
        || lower.starts_with("b'")
        || lower.starts_with("0x")
        || lower.starts_with("current_timestamp")
        || lower.ends_with(')');
    if plain {
        format!("DEFAULT {value}")
    } else {
        format!("DEFAULT '{}'", value.replace('\'', "''"))
    }
}

fn my_column_definition(row: &[String]) -> ColumnDef {
    let field = |index: usize| row.get(index).map_or("", String::as_str);
    let mut extra: Vec<String> = field(5)
        .split_whitespace()
        .map(|word| match word.to_lowercase().as_str() {
            "auto_increment" | "on" | "update" | "virtual" | "stored" | "persistent"
            | "generated" | "invisible" | "default_generated" => word.to_uppercase(),
            _ => word.to_string(),
        })
        .collect();
    let expression = extra.iter().any(|word| word == "DEFAULT_GENERATED");
    extra.retain(|word| word != "DEFAULT_GENERATED");
    let generation = field(7);
    let computed = !generation.is_empty() && extra.iter().any(|word| word == "GENERATED");
    if computed {
        extra.retain(|word| word != "GENERATED");
        extra.insert(
            0,
            format!("GENERATED ALWAYS AS {}", parenthesized(generation)),
        );
    }
    let not_null = if field(2) == "NO" { "NOT NULL" } else { "" };
    let default = if field(3) == "1" && !computed && !field(4).eq_ignore_ascii_case("null") {
        my_default(field(4), field(1), expression)
    } else {
        String::new()
    };
    let key = match field(6) {
        "PRI" => "PRIMARY KEY",
        "UNI" => "UNIQUE",
        _ => "",
    };
    ColumnDef {
        name: field(0).to_string(),
        definition: column_definition(&[field(1), not_null, &default, &extra.join(" "), key]),
    }
}

fn my_type_name(column: &mysql::Column) -> String {
    format!("{:?}", column.column_type())
        .trim_start_matches("MYSQL_TYPE_")
        .to_lowercase()
}

impl Backend for MyBackend {
    fn server_info(&mut self) -> String {
        let version = self
            .strings("SELECT VERSION()", ())
            .ok()
            .and_then(|rows| rows.into_iter().flatten().next())
            .unwrap_or_default();
        if version.to_lowercase().contains("mariadb") {
            format!("MariaDB {version}")
        } else {
            format!("MySQL {version}").trim_end().to_string()
        }
    }

    fn schemas(&mut self) -> Result<Vec<String>, Failure> {
        let mut schemas: Vec<String> = self
            .strings("SHOW DATABASES", ())?
            .into_iter()
            .flatten()
            .collect();
        let database = self.database.clone();
        schemas.sort_by_cached_key(|name| {
            let rank = if *name == database {
                0
            } else if MYSQL_SYSTEM_SCHEMAS.contains(&name.to_lowercase().as_str()) {
                2
            } else {
                1
            };
            (rank, name.to_lowercase())
        });
        Ok(schemas)
    }

    fn tables(&mut self, schema: &str) -> Result<Vec<TableInfo>, Failure> {
        let rows = self.strings(
            "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
            (schema,),
        )?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let mut row = row.into_iter();
                let name = row.next()?;
                let kind = if row.next()?.to_uppercase().contains("VIEW") {
                    TableKind::View
                } else {
                    TableKind::Table
                };
                Some(TableInfo { name, kind })
            })
            .collect())
    }

    fn rows(
        &mut self,
        schema: &str,
        table: &str,
        offset: u64,
        limit: u32,
    ) -> Result<TableRows, Failure> {
        use mysql::prelude::Queryable;
        let primary_key: Vec<String> = self
            .strings(
                "SELECT COLUMN_NAME FROM information_schema.KEY_COLUMN_USAGE \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY' \
                 ORDER BY ORDINAL_POSITION",
                (schema, table),
            )?
            .into_iter()
            .flatten()
            .collect();
        let types: Vec<String> = self
            .strings(
                "SELECT COLUMN_TYPE FROM information_schema.COLUMNS \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
                (schema, table),
            )?
            .into_iter()
            .flatten()
            .collect();
        let sql = select_sql(DbKind::MySql, schema, table, &primary_key, offset, limit)?;
        let mut result = self.conn.query_iter(sql).map_err(my_error)?;
        let described: Vec<mysql::Column> = result.columns().as_ref().to_vec();
        let binary: Vec<bool> = described.iter().map(my_is_binary).collect();
        let mut rows = Vec::new();
        for row in result.by_ref() {
            rows.push(my_cells(row.map_err(my_error)?, &binary));
        }
        let known = types.len() == described.len();
        let columns = described
            .iter()
            .enumerate()
            .map(|(i, column)| ColumnInfo {
                name: column.name_str().into_owned(),
                type_name: if known {
                    types[i].clone()
                } else {
                    my_type_name(column)
                },
            })
            .collect();
        Ok(TableRows {
            columns,
            rows,
            primary_key,
        })
    }

    fn count(&mut self, schema: &str, table: &str) -> Result<u64, Failure> {
        use mysql::prelude::Queryable;
        let sql = count_sql(DbKind::MySql, schema, table)?;
        let row: Option<mysql::Row> = self.conn.query_first(sql).map_err(my_error)?;
        let text = row
            .and_then(|row| row.unwrap().into_iter().next())
            .map(my_plain_text)
            .ok_or("count query returned no rows")?;
        text.parse()
            .map_err(|_| format!("unexpected count {text:?}").into())
    }

    fn ddl(&mut self, schema: &str, table: &str) -> Result<TableDdl, Failure> {
        let sql = format!(
            "SHOW CREATE TABLE {}",
            qualified(DbKind::MySql, schema, table)?
        );
        let created = self.strings(&sql, ())?;
        let Some(create) = created
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().nth(1))
        else {
            return Err(no_definition(schema, table));
        };
        let rows = self.strings(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT IS NOT NULL, \
                    COLUMN_DEFAULT, EXTRA, COLUMN_KEY, GENERATION_EXPRESSION \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
            (schema, table),
        )?;
        Ok(TableDdl {
            sql: join_statements([create]),
            columns: rows.iter().map(|row| my_column_definition(row)).collect(),
        })
    }

    fn script(&mut self, sql: &str, limit: u32) -> Result<ScriptResult, Failure> {
        use mysql::prelude::Queryable;
        let mut out = ScriptResult::default();
        let mut result = self.conn.query_iter(sql).map_err(my_error)?;
        while let Some(set) = result.iter() {
            out.statements += 1;
            let described: Vec<mysql::Column> = set.columns().as_ref().to_vec();
            let binary: Vec<bool> = described.iter().map(my_is_binary).collect();
            if described.is_empty() {
                out.affected += set.affected_rows();
            } else {
                out.begin_set(
                    described
                        .iter()
                        .map(|column| ColumnInfo {
                            name: column.name_str().into_owned(),
                            type_name: my_type_name(column),
                        })
                        .collect(),
                );
            }
            for row in set {
                let row = row.map_err(my_error)?;
                out.push(limit, || my_cells(row, &binary));
            }
        }
        Ok(out)
    }

    fn update_row(&mut self, update: &RowUpdate) -> Result<(), Failure> {
        use mysql::prelude::Queryable;
        let sql = update_sql(DbKind::MySql, update, placeholder)?;
        let params: Vec<mysql::Value> = update_params(update)
            .into_iter()
            .map(|text| mysql::Value::Bytes(text.as_bytes().to_vec()))
            .collect();
        let mut transaction = self
            .conn
            .start_transaction(mysql::TxOpts::default())
            .map_err(my_error)?;
        transaction.exec_drop(&sql, params).map_err(my_error)?;
        let matched =
            my_rows_matched(&transaction.info_str()).unwrap_or_else(|| transaction.affected_rows());
        matched_one(matched)?;
        transaction.commit().map_err(my_error)
    }

    fn is_broken(&mut self) -> bool {
        self.conn.ping().is_err()
    }

    fn canceller(&self) -> Option<Cancel> {
        let id = self.conn.connection_id();
        let opts = mysql::Opts::from(
            mysql::OptsBuilder::from_opts(self.opts.clone())
                .db_name(None::<String>)
                .read_timeout(Some(CONNECT_TIMEOUT))
                .write_timeout(Some(CONNECT_TIMEOUT)),
        );
        Some(Arc::new(move || {
            use mysql::prelude::Queryable;
            if let Ok(mut killer) = mysql::Conn::new(opts.clone()) {
                let _ = killer.query_drop(format!("KILL QUERY {id}"));
            }
        }))
    }
}

struct SqliteBackend {
    conn: rusqlite::Connection,
}

impl SqliteBackend {
    fn open(path: &str, halt: Arc<AtomicBool>, writable: bool) -> Result<Self, String> {
        use rusqlite::OpenFlags;
        if path.trim().is_empty() {
            return Err("no database file specified".to_string());
        }
        let path = expand_home(path.trim());
        if !path.is_file() {
            return Err(format!("{}: no such database file", path.display()));
        }
        let base = OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut conn =
            rusqlite::Connection::open_with_flags(&path, base | OpenFlags::SQLITE_OPEN_READ_WRITE)
                .or_else(|_| {
                    rusqlite::Connection::open_with_flags(
                        &path,
                        base | OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                })
                .map_err(|err| format!("{}: {err}", path.display()))?;
        let _ = conn.busy_timeout(Duration::from_secs(5));
        let watch = move || halt.load(Ordering::Relaxed);
        conn.progress_handler(SQLITE_PROGRESS_OPS, Some(watch))
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let guarded = if writable {
            Ok(())
        } else {
            conn.execute_batch("PRAGMA query_only = ON")
        };
        guarded
            .and_then(|()| conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(())))
            .map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(Self { conn })
    }

    fn primary_key(&self, schema: &str, table: &str) -> Result<Vec<String>, Failure> {
        let mut statement = self
            .conn
            .prepare("SELECT name FROM pragma_table_info(?1, ?2) WHERE pk > 0 ORDER BY pk")
            .map_err(sqlite_error)?;
        let names = statement
            .query_map([table, schema], |row| row.get::<_, String>(0))
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        Ok(names)
    }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

fn sqlite_error(err: rusqlite::Error) -> Failure {
    Failure {
        message: err.to_string(),
        cancelled: err.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted),
    }
}

fn sqlite_columns(statement: &rusqlite::Statement<'_>) -> Vec<ColumnInfo> {
    statement
        .columns()
        .map(|column| ColumnInfo {
            name: column.name().to_string(),
            type_name: column.decl_type().unwrap_or_default().to_lowercase(),
        })
        .collect()
}

fn sqlite_cells(row: &rusqlite::Row<'_>, width: usize) -> Vec<Field> {
    (0..width)
        .map(|i| row.get_ref(i).map(sqlite_render).unwrap_or_default())
        .collect()
}

fn sqlite_render(value: rusqlite::types::ValueRef<'_>) -> Field {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Field::default(),
        ValueRef::Integer(n) => Field::shown(n.to_string()),
        ValueRef::Real(n) => Field::shown(n.to_string()),
        ValueRef::Text(bytes) => Field::text(&String::from_utf8_lossy(bytes)),
        ValueRef::Blob(bytes) => Field::opaque(render_bytes(bytes)),
    }
}

impl Backend for SqliteBackend {
    fn server_info(&mut self) -> String {
        format!("SQLite {}", rusqlite::version())
    }

    fn schemas(&mut self) -> Result<Vec<String>, Failure> {
        let mut statement = self
            .conn
            .prepare("PRAGMA database_list")
            .map_err(sqlite_error)?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        Ok(names)
    }

    fn tables(&mut self, schema: &str) -> Result<Vec<TableInfo>, Failure> {
        let sql = format!(
            "SELECT name, type FROM {} \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             ORDER BY name",
            qualified(DbKind::Sqlite, schema, "sqlite_master")?
        );
        let mut statement = self.conn.prepare(&sql).map_err(sqlite_error)?;
        let tables = statement
            .query_map([], |row| {
                Ok(TableInfo {
                    name: row.get(0)?,
                    kind: if row.get::<_, String>(1)? == "view" {
                        TableKind::View
                    } else {
                        TableKind::Table
                    },
                })
            })
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        Ok(tables)
    }

    fn rows(
        &mut self,
        schema: &str,
        table: &str,
        offset: u64,
        limit: u32,
    ) -> Result<TableRows, Failure> {
        let primary_key = self.primary_key(schema, table)?;
        let sql = select_sql(DbKind::Sqlite, schema, table, &primary_key, offset, limit)?;
        let mut statement = self.conn.prepare(&sql).map_err(sqlite_error)?;
        let columns = sqlite_columns(&statement);
        let mut rows = Vec::new();
        let mut cursor = statement.query([]).map_err(sqlite_error)?;
        while let Some(row) = cursor.next().map_err(sqlite_error)? {
            rows.push(sqlite_cells(row, columns.len()));
        }
        Ok(TableRows {
            columns,
            rows,
            primary_key,
        })
    }

    fn count(&mut self, schema: &str, table: &str) -> Result<u64, Failure> {
        let sql = count_sql(DbKind::Sqlite, schema, table)?;
        self.conn
            .query_row(&sql, [], |row| row.get::<_, i64>(0))
            .map(|count| count.max(0) as u64)
            .map_err(sqlite_error)
    }

    fn ddl(&mut self, schema: &str, table: &str) -> Result<TableDdl, Failure> {
        let sql = format!(
            "SELECT sql FROM {} WHERE tbl_name = ?1 AND sql IS NOT NULL \
             ORDER BY type NOT IN ('table', 'view'), type, name",
            qualified(DbKind::Sqlite, schema, "sqlite_master")?
        );
        let mut statement = self.conn.prepare(&sql).map_err(sqlite_error)?;
        let statements = statement
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        if statements.is_empty() {
            return Err(no_definition(schema, table));
        }
        let mut statement = self
            .conn
            .prepare(
                "SELECT name, type, \"notnull\", dflt_value, pk, hidden \
                 FROM pragma_table_xinfo(?1, ?2) ORDER BY cid",
            )
            .map_err(sqlite_error)?;
        let columns = statement
            .query_map([table, schema], |row| {
                let default: Option<String> = row.get(3)?;
                let default = default.map(|value| format!("DEFAULT {value}"));
                let not_null = if row.get::<_, i64>(2)? != 0 {
                    "NOT NULL"
                } else {
                    ""
                };
                let primary = if row.get::<_, i64>(4)? > 0 {
                    "PRIMARY KEY"
                } else {
                    ""
                };
                let generated = match row.get::<_, i64>(5)? {
                    2 => "GENERATED ALWAYS VIRTUAL",
                    3 => "GENERATED ALWAYS STORED",
                    _ => "",
                };
                Ok(ColumnDef {
                    name: row.get(0)?,
                    definition: column_definition(&[
                        &row.get::<_, String>(1)?,
                        not_null,
                        default.as_deref().unwrap_or_default(),
                        generated,
                        primary,
                    ]),
                })
            })
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        Ok(TableDdl {
            sql: join_statements(statements),
            columns,
        })
    }

    fn script(&mut self, sql: &str, limit: u32) -> Result<ScriptResult, Failure> {
        use rusqlite::fallible_iterator::FallibleIterator;
        let mut out = ScriptResult::default();
        let mut batch = rusqlite::Batch::new(&self.conn, sql);
        while let Some(mut statement) = batch.next().map_err(sqlite_error)? {
            out.statements += 1;
            let width = statement.column_count();
            if width > 0 {
                out.begin_set(sqlite_columns(&statement));
            }
            let skippable = statement.readonly();
            let changes = self.conn.total_changes();
            let mut cursor = statement.query([]).map_err(sqlite_error)?;
            while let Some(row) = cursor.next().map_err(sqlite_error)? {
                if skippable && out.is_full(limit) {
                    out.truncated = true;
                    break;
                }
                out.push(limit, || sqlite_cells(row, width));
            }
            out.affected += self.conn.total_changes() - changes;
        }
        Ok(out)
    }

    fn update_row(&mut self, update: &RowUpdate) -> Result<(), Failure> {
        let sql = update_sql(DbKind::Sqlite, update, placeholder)?;
        let transaction = self.conn.transaction().map_err(sqlite_error)?;
        let matched = transaction
            .execute(&sql, rusqlite::params_from_iter(update_params(update)))
            .map_err(sqlite_error)?;
        matched_one(matched as u64)?;
        transaction.commit().map_err(sqlite_error)
    }

    fn is_broken(&mut self) -> bool {
        false
    }

    fn canceller(&self) -> Option<Cancel> {
        None
    }
}

#[cfg(test)]
mod tests;
