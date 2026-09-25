#![allow(dead_code)] // Shared by integration test binaries with different scenarios.

use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use llmproxy_core::protocol::Protocol;
use llmproxy_store::{ProviderInput, ProviderPaths, ProviderStore};

pub const DEADLINE: Duration = Duration::from_secs(8);
pub const PATHS: [&str; 3] = ["/v1/chat/completions", "/v1/responses", "/v1/messages"];
const PROTOCOLS: [Protocol; 3] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
];
pub const SECRETS: [&str; 3] = ["dummy-chat", "dummy-responses", "dummy-messages"];
const MASTER_KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
// Hold this through gateway startup: another test must not acquire the released
// reservation while the child process is still initializing its listener.
static PORT_ALLOCATION: Mutex<()> = Mutex::new(());

pub fn bind_listener(address: impl ToSocketAddrs) -> TcpListener {
    let _allocation = PORT_ALLOCATION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    TcpListener::bind(address).unwrap()
}

pub struct Gateway {
    child: Child,
    pub address: SocketAddr,
    directory: PathBuf,
    database: Option<TestDatabase>,
}

struct TestDatabase {
    url: String,
}

impl TestDatabase {
    fn new(directory: &std::path::Path) -> Self {
        Self {
            url: format!("sqlite:{}", directory.join("providers.sqlite3").display()),
        }
    }

    fn populate(&self, providers: [Provider; 3]) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(async {
            let store = ProviderStore::connect(&self.url, MASTER_KEY)
                .await
                .map_err(|_| "connect isolated provider store")?;
            store
                .migrate()
                .await
                .map_err(|_| "migrate isolated database")?;
            for (index, provider) in providers.into_iter().enumerate() {
                let host = provider.host.unwrap_or(if provider.tls {
                    "localhost"
                } else {
                    "127.0.0.1"
                });
                let record = store
                    .create(ProviderInput {
                        name: format!("Test {}", PROTOCOLS[index].as_str()),
                        paths: ProviderPaths::single(PROTOCOLS[index]),
                        host: host.to_owned(),
                        port: provider.address.port(),
                        tls: provider.tls,
                        api_key: SECRETS[index].to_owned(),
                        enabled: true,
                        models_path: "/models".into(),
                        models_protocol: PROTOCOLS[index],
                        anthropic_version: provider.version.map(str::to_owned),
                        connect_timeout_ms: provider.connect_ms,
                        read_timeout_ms: provider.read_ms,
                        write_timeout_ms: provider.write_ms,
                    })
                    .await
                    .map_err(|_| "create test provider")?;
                store
                    .activate(record.id, record.version, PROTOCOLS[index])
                    .await
                    .map_err(|_| "activate test provider")?;
            }
            Ok::<(), &'static str>(())
        });
        drop(runtime);
        result.expect("populate isolated provider database");
    }
}

#[derive(Clone, Copy)]
pub struct Provider {
    pub address: SocketAddr,
    pub host: Option<&'static str>,
    pub tls: bool,
    pub version: Option<&'static str>,
    pub connect_ms: u64,
    pub read_ms: u64,
    pub write_ms: u64,
}

impl Provider {
    pub fn http(address: SocketAddr) -> Self {
        Self {
            address,
            host: None,
            tls: false,
            version: Some("2023-06-01"),
            connect_ms: 1500,
            read_ms: 1500,
            write_ms: 1500,
        }
    }
}

impl Gateway {
    pub fn start(providers: [Provider; 3]) -> Self {
        let _allocation = PORT_ALLOCATION
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-integration-{}-{}",
            std::process::id(),
            id,
        ));
        fs::create_dir(&directory).unwrap();
        let database = TestDatabase::new(&directory);
        database.populate(providers);
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut command = Self::command(&directory);
        command
            .env("LLMPROXY_DATABASE_URL", &database.url)
            .env("LLMPROXY_MASTER_KEY", MASTER_KEY)
            .env(
                "LLMPROXY_LISTEN",
                reservation.local_addr().unwrap().to_string(),
            );
        Self::launch(command, reservation, directory, id, Some(database))
    }

    pub fn database(url: &str, master_key: &str) -> Self {
        let _allocation = PORT_ALLOCATION
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-database-integration-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let mut command = Self::command(&directory);
        command
            .env("LLMPROXY_DATABASE_URL", url)
            .env("LLMPROXY_MASTER_KEY", master_key)
            .env(
                "LLMPROXY_LISTEN",
                reservation.local_addr().unwrap().to_string(),
            );
        Self::launch(command, reservation, directory, id, None)
    }

    fn command(directory: &std::path::Path) -> Command {
        let log = File::create(directory.join("gateway.log")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_llmproxy"));
        // Do not inherit real credentials, exporters, proxy settings, or log filters.
        command
            .env_clear()
            .current_dir(directory)
            .env(
                "RUST_LOG",
                "llmproxy=info,llmproxy_gateway=info,pingora=warn",
            )
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        command
    }

    fn launch(
        mut command: Command,
        reservation: TcpListener,
        directory: PathBuf,
        id: usize,
        database: Option<TestDatabase>,
    ) -> Self {
        let address = reservation.local_addr().unwrap();
        drop(reservation);
        let child = command.spawn().unwrap();
        let mut gateway = Self {
            child,
            address,
            directory,
            database,
        };
        let deadline = Instant::now() + DEADLINE;
        let readiness_path = format!("/__llmproxy_test_ready/{}/{id}", std::process::id());
        let mut probe_sent = false;
        loop {
            if let Some(status) = gateway.child.try_wait().unwrap() {
                panic!("gateway exited {status}: {}", gateway.logs());
            }
            let logs = gateway.logs();
            // A TCP connection alone could hit a concurrent test's reservation.
            // Require an HTTP response and this child's unique request log.
            if probe_sent && logs.contains(&readiness_path) {
                return gateway;
            }
            if !probe_sent && logs.contains("gateway listening") {
                probe_sent = gateway.probe_ready(&readiness_path);
            }
            assert!(
                Instant::now() < deadline,
                "gateway not ready: {}",
                gateway.logs()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn probe_ready(&self, path: &str) -> bool {
        let probe = || -> io::Result<bool> {
            let timeout = Duration::from_millis(200);
            let mut stream = TcpStream::connect_timeout(&self.address, timeout)?;
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
            write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: readiness.invalid\r\nConnection: close\r\n\r\n"
            )?;
            Ok(Response::read(stream)?.status == 404)
        };
        probe().unwrap_or(false)
    }

    pub fn connect(&self) -> TcpStream {
        let stream = TcpStream::connect_timeout(&self.address, DEADLINE).unwrap();
        stream.set_read_timeout(Some(DEADLINE)).unwrap();
        stream.set_write_timeout(Some(DEADLINE)).unwrap();
        stream.set_nodelay(true).unwrap();
        stream
    }

    pub fn request(&self, method: &str, path: &str, headers: &str, body: &[u8]) -> Response {
        let mut stream = self.connect();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: caller.invalid\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n", body.len()).unwrap();
        stream.write_all(body).unwrap();
        Response::read(stream).unwrap()
    }

    pub fn logs(&self) -> String {
        fs::read_to_string(self.directory.join("gateway.log")).unwrap_or_default()
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if thread::panicking() {
            eprintln!("gateway diagnostics:\n{}", self.logs());
        }
        let _ = fs::remove_dir_all(&self.directory);
        drop(self.database.take());
    }
}

pub struct Mock {
    pub address: SocketAddr,
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<TcpStream>>>,
    thread: Option<JoinHandle<()>>,
}

impl Mock {
    pub fn raw(handler: impl Fn(TcpStream) + Send + Sync + 'static) -> Self {
        let listener = bind_listener("127.0.0.1:0");
        Self::on_listener(listener, handler)
    }

    pub fn on_listener(
        listener: TcpListener,
        handler: impl Fn(TcpStream) + Send + Sync + 'static,
    ) -> Self {
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let (worker_count, worker_stop, worker_sockets) =
            (count.clone(), stop.clone(), sockets.clone());
        let handler = Arc::new(handler);
        let thread = thread::spawn(move || {
            let mut workers = Vec::new();
            while !worker_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // macOS inherits the listener's nonblocking mode on accept.
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(DEADLINE)).unwrap();
                        stream.set_write_timeout(Some(DEADLINE)).unwrap();
                        stream.set_nodelay(true).unwrap();
                        worker_sockets
                            .lock()
                            .unwrap()
                            .push(stream.try_clone().unwrap());
                        worker_count.fetch_add(1, Ordering::SeqCst);
                        let handler = handler.clone();
                        workers.push(thread::spawn(move || {
                            let connection = stream.try_clone().unwrap();
                            handler(stream);
                            let _ = connection.shutdown(Shutdown::Both);
                        }));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mock accept: {error}"),
                }
            }
            for stream in worker_sockets.lock().unwrap().iter() {
                let _ = stream.shutdown(Shutdown::Both);
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            address,
            count,
            stop,
            sockets,
            thread: Some(thread),
        }
    }

    pub fn http(
        handler: impl Fn(&Request, &mut TcpStream) + Send + Sync + 'static,
    ) -> (Self, Receiver<Request>) {
        let (sender, receiver) = mpsc::channel();
        let mock = Self::raw(move |mut stream| {
            let request = Request::read(&mut stream).unwrap();
            // Some tests only inspect the response and discard the capture channel.
            let _ = sender.send(request.clone());
            handler(&request, &mut stream);
        });
        (mock, receiver)
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Shutdown wakes blocking reads even if a test assertion already failed.
        for socket in self.sockets.lock().unwrap().iter() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}

pub type Headers = Vec<(String, String)>;

pub fn values<'a>(headers: &'a Headers, name: &str) -> Vec<&'a str> {
    headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .collect()
}

fn line(reader: &mut impl BufRead) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader.read_until(b'\n', &mut bytes)?;
    if !bytes.ends_with(b"\r\n") {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "incomplete HTTP line",
        ));
    }
    bytes.truncate(bytes.len() - 2);
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn headers(reader: &mut impl BufRead) -> io::Result<Headers> {
    let mut result = Vec::new();
    loop {
        let line = line(reader)?;
        if line.is_empty() {
            return Ok(result);
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid HTTP header"))?;
        result.push((name.to_owned(), value.trim().to_owned()));
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Request {
    pub fn read(stream: &mut TcpStream) -> io::Result<Self> {
        let mut reader = BufReader::new(stream);
        let request_line = line(&mut reader)?;
        let parts: Vec<_> = request_line.split_whitespace().collect();
        let headers = headers(&mut reader)?;
        let length = values(&headers, "content-length")
            .first()
            .map(|value| value.parse::<usize>().unwrap())
            .unwrap_or(0);
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        Ok(Self {
            method: parts[0].into(),
            target: parts[1].into(),
            headers,
            body,
        })
    }
}

enum Framing {
    Chunked,
    Length(usize),
    Close,
}

pub struct Response {
    pub status: u16,
    pub headers: Headers,
    reader: BufReader<TcpStream>,
    framing: Framing,
    pending: VecDeque<u8>,
    complete: bool,
}

impl Response {
    pub fn read(stream: TcpStream) -> io::Result<Self> {
        let mut reader = BufReader::new(stream);
        let status_line = line(&mut reader)?;
        let status = status_line
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = headers(&mut reader)?;
        let framing = if values(&headers, "transfer-encoding")
            .iter()
            .any(|value| value.eq_ignore_ascii_case("chunked"))
        {
            Framing::Chunked
        } else if let Some(length) = values(&headers, "content-length").first() {
            Framing::Length(length.parse().unwrap())
        } else {
            Framing::Close
        };
        Ok(Self {
            status,
            headers,
            reader,
            framing,
            pending: VecDeque::new(),
            complete: false,
        })
    }

    pub fn next_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        if !self.pending.is_empty() {
            return Ok(Some(self.pending.drain(..).collect()));
        }
        if self.complete {
            return Ok(None);
        }
        let size = match self.framing {
            Framing::Chunked => {
                let line = line(&mut self.reader)?;
                usize::from_str_radix(line.split(';').next().unwrap(), 16)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            }
            Framing::Length(remaining) => remaining.min(65536),
            Framing::Close => {
                let mut bytes = vec![0; 65536];
                let read = self.reader.read(&mut bytes)?;
                bytes.truncate(read);
                if read == 0 {
                    self.complete = true;
                    return Ok(None);
                }
                return Ok(Some(bytes));
            }
        };
        if size == 0 {
            if matches!(self.framing, Framing::Chunked) {
                headers(&mut self.reader)?;
            }
            self.complete = true;
            return Ok(None);
        }
        let mut bytes = vec![0; size];
        self.reader.read_exact(&mut bytes)?;
        match &mut self.framing {
            Framing::Chunked => {
                if !line(&mut self.reader)?.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid chunk ending",
                    ));
                }
            }
            Framing::Length(remaining) => *remaining -= size,
            Framing::Close => unreachable!(),
        }
        Ok(Some(bytes))
    }

    pub fn bytes(&mut self, count: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        while bytes.len() < count {
            let chunk = self.next_chunk().unwrap().expect("body ended early");
            let take = (count - bytes.len()).min(chunk.len());
            bytes.extend_from_slice(&chunk[..take]);
            self.pending.extend(&chunk[take..]);
        }
        bytes
    }

    pub fn body(mut self) -> Vec<u8> {
        let mut result = Vec::new();
        while let Some(chunk) = self.next_chunk().unwrap() {
            result.extend(chunk);
        }
        result
    }

    pub fn cancel(self) {
        self.reader.get_ref().shutdown(Shutdown::Both).unwrap();
    }
}

pub fn respond(stream: &mut TcpStream, status: u16, headers: &str, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status} Mock\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();
}

pub fn sse_headers(stream: &mut TcpStream) {
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\nCache-Control: no-cache\r\n\r\n").unwrap();
    stream.flush().unwrap();
}

pub fn chunk(stream: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
    write!(stream, "{:x}\r\n", bytes.len())?;
    stream.write_all(bytes)?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}

pub fn finish_chunks(stream: &mut TcpStream) {
    stream.write_all(b"0\r\n\r\n").unwrap();
    stream.flush().unwrap();
}
