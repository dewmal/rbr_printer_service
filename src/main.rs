#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use anyhow::{bail, Context, Result};
use eframe::egui;
use lopdf::{Dictionary, Document, Object, Stream};
use reqwest::blocking::Client;
use serde_json::Value;

use std::{
    env,
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom, Write},
    panic::{self, AssertUnwindSafe},
    path::Path,
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use tempfile::{Builder, NamedTempFile};
use tungstenite::{connect, Message};
use url::Url;

// =========================================================
// CONSTANTS
// =========================================================

const MAX_PDF_SIZE: u64 = 100 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

// Thermal paper geometry.
// The physical roll is 76 mm wide. We reserve 2.5 mm on both sides,
// so the source PDF content is uniformly scaled to a 71 mm content area.
const PAPER_WIDTH_MM: f64 = 76.0;
const SIDE_MARGIN_MM: f64 = 2.5;
const CONTENT_WIDTH_MM: f64 = PAPER_WIDTH_MM - (SIDE_MARGIN_MM * 2.0);

// =========================================================
// CONFIGURATION
// =========================================================

#[derive(Clone)]
struct Config {
    socket_server: String,
    socket_print_event: String,
}

// =========================================================
// GUI STATUS
// =========================================================

#[derive(Clone, Debug)]
enum PrinterStatus {
    Starting,
    Connecting,
    Connected,
    Waiting,
    Downloading,
    Printing,
    Success,
    Error,
    Disconnected,
}

// =========================================================
// GUI EVENTS
// =========================================================

enum WorkerEvent {
    Status(PrinterStatus),
    Server(String),
    SocketEvent(String),
    Url(String),
    Message(String),
}

// =========================================================
// PRINT JOB
// =========================================================

struct PrintJob {
    url: String,
}

// =========================================================
// GUI
// =========================================================

struct PrinterApp {
    status: PrinterStatus,

    server: String,

    socket_event: String,

    last_url: String,

    message: String,

    rx: Receiver<WorkerEvent>,
}

// =========================================================
// GUI IMPLEMENTATION
// =========================================================

impl PrinterApp {
    fn new(
        rx: Receiver<WorkerEvent>,
    ) -> Self {
        Self {
            status: PrinterStatus::Starting,

            server: String::new(),

            socket_event: String::new(),

            last_url: String::new(),

            message:
            "Starting application..."
                .to_string(),

            rx,
        }
    }

    fn process_events(
        &mut self,
    ) {
        while let Ok(event) =
            self.rx.try_recv()
        {
            match event {
                WorkerEvent::Status(
                    status,
                ) => {
                    self.status =
                        status;
                }

                WorkerEvent::Server(
                    server,
                ) => {
                    self.server =
                        server;
                }

                WorkerEvent::SocketEvent(
                    event,
                ) => {
                    self.socket_event =
                        event;
                }

                WorkerEvent::Url(
                    url,
                ) => {
                    self.last_url =
                        url;
                }

                WorkerEvent::Message(
                    message,
                ) => {
                    self.message =
                        message;
                }
            }
        }
    }

    fn status_text(
        &self,
    ) -> &'static str {
        match self.status {
            PrinterStatus::Starting =>
                "Starting",

            PrinterStatus::Connecting =>
                "Connecting",

            PrinterStatus::Connected =>
                "Connected",

            PrinterStatus::Waiting =>
                "Waiting",

            PrinterStatus::Downloading =>
                "Downloading",

            PrinterStatus::Printing =>
                "Printing",

            PrinterStatus::Success =>
                "Printed",

            PrinterStatus::Error =>
                "Error",

            PrinterStatus::Disconnected =>
                "Disconnected",
        }
    }

    fn status_color(
        &self,
    ) -> egui::Color32 {
        match self.status {
            PrinterStatus::Connected
            | PrinterStatus::Waiting
            | PrinterStatus::Success => {
                egui::Color32::from_rgb(
                    40,
                    180,
                    90,
                )
            }

            PrinterStatus::Starting
            | PrinterStatus::Connecting
            | PrinterStatus::Downloading
            | PrinterStatus::Printing => {
                egui::Color32::from_rgb(
                    230,
                    170,
                    40,
                )
            }

            PrinterStatus::Error
            | PrinterStatus::Disconnected => {
                egui::Color32::from_rgb(
                    220,
                    60,
                    60,
                )
            }
        }
    }
}

// =========================================================
// EFRAME APP
// =========================================================

impl eframe::App for PrinterApp {
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _frame: &mut eframe::Frame,
    ) {
        self.process_events();

        egui::CentralPanel::default()
            .show(
                ui,
                |ui| {
                    ui.add_space(
                        10.0,
                    );

                    ui.heading(
                        "PDF Printer Client",
                    );

                    ui.add_space(
                        15.0,
                    );

                    // -----------------------------
                    // STATUS
                    // -----------------------------

                    ui.horizontal(
                        |ui| {
                            ui.colored_label(
                                self.status_color(),
                                egui::RichText::new(
                                    "●",
                                )
                                    .size(
                                        22.0,
                                    ),
                            );

                            ui.label(
                                egui::RichText::new(
                                    self.status_text(),
                                )
                                    .strong()
                                    .size(
                                        18.0,
                                    ),
                            );
                        },
                    );

                    ui.add_space(
                        15.0,
                    );

                    ui.separator();

                    ui.add_space(
                        15.0,
                    );

                    // -----------------------------
                    // SERVER
                    // -----------------------------

                    ui.label(
                        egui::RichText::new(
                            "Socket.IO Server",
                        )
                            .strong(),
                    );

                    if self.server.is_empty() {
                        ui.label("-");
                    } else {
                        ui.monospace(
                            &self.server,
                        );
                    }

                    ui.add_space(
                        15.0,
                    );

                    // -----------------------------
                    // EVENT
                    // -----------------------------

                    ui.label(
                        egui::RichText::new(
                            "Listening Event",
                        )
                            .strong(),
                    );

                    if self
                        .socket_event
                        .is_empty()
                    {
                        ui.label("-");
                    } else {
                        ui.monospace(
                            &self.socket_event,
                        );
                    }

                    ui.add_space(
                        15.0,
                    );

                    // -----------------------------
                    // STATUS MESSAGE
                    // -----------------------------

                    ui.label(
                        egui::RichText::new(
                            "Status",
                        )
                            .strong(),
                    );

                    ui.label(
                        &self.message,
                    );

                    ui.add_space(
                        15.0,
                    );

                    // -----------------------------
                    // LAST PDF
                    // -----------------------------

                    ui.label(
                        egui::RichText::new(
                            "Last PDF URL",
                        )
                            .strong(),
                    );

                    if self
                        .last_url
                        .is_empty()
                    {
                        ui.label(
                            "No print jobs received yet.",
                        );
                    } else {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    &self.last_url,
                                )
                                    .monospace()
                                    .small(),
                            )
                                .wrap(),
                        );
                    }

                    ui.add_space(
                        15.0,
                    );

                    ui.separator();

                    ui.add_space(
                        8.0,
                    );

                    ui.small(
                        "Socket.IO v1 / Engine.IO v3 compatibility mode",
                    );
                },
            );

        ui.ctx()
            .request_repaint_after(
                Duration::from_millis(
                    100,
                ),
            );
    }
}

// =========================================================
// MAIN
// =========================================================

fn main() -> eframe::Result<()> {
    install_crash_logger();

    log_info("[APP] RBPrinter starting");

    let (ui_tx, ui_rx) =
        mpsc::channel::<WorkerEvent>();

    let worker_ui_tx = ui_tx.clone();

    thread::spawn(move || {
        log_info("[THREAD] service worker starting");

        let worker_result = panic::catch_unwind(
            AssertUnwindSafe(|| {
                service_worker(worker_ui_tx.clone());
            }),
        );

        if worker_result.is_err() {
            log_error("[FATAL] service worker panicked");

            send_status(
                &worker_ui_tx,
                PrinterStatus::Error,
                "Background service crashed. Check rbprinter-crash.log and rbprinter.log.",
            );
        }
    });

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,

        run_and_return: false,

        viewport: egui::ViewportBuilder::default()
            .with_title("PDF Printer Client")
            .with_inner_size([500.0, 450.0])
            .with_min_inner_size([500.0, 450.0])
            .with_resizable(false),

        ..Default::default()
    };

    log_info("[APP] starting GUI");

    let result = eframe::run_native(
        "PDF Printer Client",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(
                PrinterApp::new(ui_rx),
            ))
        }),
    );

    if let Err(err) = &result {
        log_error(&format!(
            "[GUI ERROR] {err:?}"
        ));
    }

    result
}

// =========================================================
// SERVICE WORKER
// =========================================================

fn service_worker(
    ui_tx: Sender<WorkerEvent>,
) {
    send_status(
        &ui_tx,

        PrinterStatus::Starting,

        "Loading configuration...",
    );

    // ---------------------------------------------
    // CONFIG
    // ---------------------------------------------

    let config =
        match load_config() {
            Ok(config) =>
                config,

            Err(err) => {
                send_status(
                    &ui_tx,

                    PrinterStatus::Error,

                    format!(
                        "Configuration error: {err:#}"
                    ),
                );

                return;
            }
        };

    let _ =
        ui_tx.send(
            WorkerEvent::Server(
                config
                    .socket_server
                    .clone(),
            ),
        );

    let _ =
        ui_tx.send(
            WorkerEvent::SocketEvent(
                config
                    .socket_print_event
                    .clone(),
            ),
        );

    // ---------------------------------------------
    // HTTP CLIENT
    // ---------------------------------------------

    let http =
        match Client::builder()
            .timeout(
                HTTP_TIMEOUT,
            )
            .user_agent(
                "rbprinter/1.0",
            )
            .build()
        {
            Ok(client) =>
                client,

            Err(err) => {
                send_status(
                    &ui_tx,

                    PrinterStatus::Error,

                    format!(
                        "HTTP client error: {err}"
                    ),
                );

                return;
            }
        };

    // ---------------------------------------------
    // PRINT JOB QUEUE
    // ---------------------------------------------

    let (
        job_tx,
        job_rx,
    ) =
        mpsc::channel::<PrintJob>();

    let print_ui_tx =
        ui_tx.clone();

    let print_panic_ui_tx =
        print_ui_tx.clone();

    thread::spawn(move || {
        let print_result = panic::catch_unwind(
            AssertUnwindSafe(|| {
                print_worker(
                    job_rx,
                    http,
                    print_ui_tx,
                );
            }),
        );

        if print_result.is_err() {
            log_error("[FATAL] print worker panicked");

            send_status(
                &print_panic_ui_tx,
                PrinterStatus::Error,
                "Print worker crashed. Check rbprinter-crash.log and rbprinter.log.",
            );
        }
    });

    // ---------------------------------------------
    // SOCKET.IO
    // ---------------------------------------------

    let socket_panic_ui_tx =
        ui_tx.clone();

    let socket_result = panic::catch_unwind(
        AssertUnwindSafe(|| {
            socket_worker(
                config,
                job_tx,
                socket_panic_ui_tx.clone(),
            );
        }),
    );

    if socket_result.is_err() {
        log_error("[FATAL] socket worker panicked");

        send_status(
            &ui_tx,
            PrinterStatus::Error,
            "Socket worker crashed. Check rbprinter-crash.log and rbprinter.log.",
        );
    }
}

// =========================================================
// LOAD CONFIG
// =========================================================

fn load_config() -> Result<Config> {
    let exe_path =
        env::current_exe()
            .context(
                "Unable to determine executable path",
            )?;

    let exe_directory =
        exe_path
            .parent()
            .context(
                "Unable to determine executable directory",
            )?;

    // ---------------------------------------------
    // First check beside EXE
    // ---------------------------------------------

    let exe_env_path =
        exe_directory
            .join(
                ".env",
            );

    // ---------------------------------------------
    // Development fallback:
    // project directory
    // ---------------------------------------------

    let env_path =
        if exe_env_path
            .exists()
        {
            exe_env_path
        } else {
            let current_directory =
                env::current_dir()
                    .context(
                        "Unable to determine current directory",
                    )?;

            let development_path =
                current_directory
                    .join(
                        ".env",
                    );

            if development_path
                .exists()
            {
                development_path
            } else {
                bail!(
                    ".env not found.\nChecked:\n{}\n{}",
                    exe_env_path.display(),
                    development_path.display(),
                );
            }
        };

    log_info(&format!(
        "[CONFIG] .env = {}",
        env_path.display(),
    ));

    dotenvy::from_path(
        &env_path,
    )
        .with_context(
            || {
                format!(
                    "Unable to load {}",
                    env_path.display(),
                )
            },
        )?;

    // ---------------------------------------------
    // SOCKET SERVER
    // ---------------------------------------------

    let socket_server =
        env::var(
            "SOCKET_SERVER",
        )
            .context(
                "SOCKET_SERVER missing from .env",
            )?
            .trim()
            .trim_end_matches('/')
            .to_string();

    if socket_server
        .is_empty()
    {
        bail!(
            "SOCKET_SERVER cannot be empty"
        );
    }

    let server_url =
        Url::parse(
            &socket_server,
        )
            .context(
                "SOCKET_SERVER is invalid",
            )?;

    match server_url
        .scheme()
    {
        "http"
        | "https"
        | "ws"
        | "wss" => {}

        scheme => {
            bail!(
                "Unsupported server scheme: {scheme}"
            );
        }
    }

    let socket_print_event =
        env::var(
            "SOCKET_PRINT_EVENT",
        )
            .unwrap_or_else(
                |_| {
                    "chat message"
                        .to_string()
                },
            );

    Ok(
        Config {
            socket_server,

            socket_print_event,
        },
    )
}

// =========================================================
// SOCKET.IO 1.x / ENGINE.IO 3 WEBSOCKET URL
// =========================================================

fn build_socket_url(
    server_url: &str,
) -> Result<String> {
    let mut url =
        Url::parse(
            server_url,
        )?;

    match url.scheme() {
        "http" => {
            url.set_scheme(
                "ws",
            )
                .map_err(
                    |_| anyhow::anyhow!(
                    "Unable to convert http to ws"
                ),
                )?;
        }

        "https" => {
            url.set_scheme(
                "wss",
            )
                .map_err(
                    |_| anyhow::anyhow!(
                    "Unable to convert https to wss"
                ),
                )?;
        }

        "ws"
        | "wss" => {}

        scheme => {
            bail!(
                "Unsupported scheme: {scheme}"
            );
        }
    }

    // Socket.IO default path.
    url.set_path(
        "/socket.io/",
    );

    url.set_query(
        Some(
            "EIO=3&transport=websocket",
        ),
    );

    Ok(
        url.to_string(),
    )
}

// =========================================================
// SOCKET WORKER
// =========================================================

fn socket_worker(
    config: Config,

    job_tx: Sender<PrintJob>,

    ui_tx: Sender<WorkerEvent>,
) {
    loop {
        send_status(
            &ui_tx,

            PrinterStatus::Connecting,

            format!(
                "Connecting to {}...",
                config.socket_server,
            ),
        );

        let websocket_url =
            match build_socket_url(
                &config.socket_server,
            ) {
                Ok(url) =>
                    url,

                Err(err) => {
                    send_status(
                        &ui_tx,

                        PrinterStatus::Error,

                        format!(
                            "Unable to create WebSocket URL: {err:#}"
                        ),
                    );

                    thread::sleep(
                        RECONNECT_DELAY,
                    );

                    continue;
                }
            };

        log_info(&format!(
            "[SOCKET] Connecting: {}",
            websocket_url,
        ));

        // ---------------------------------------------
        // CONNECT
        // ---------------------------------------------

        log_info("[SOCKET] calling tungstenite::connect()");

        let connection =
            connect(
                websocket_url
                    .as_str(),
            );

        log_info("[SOCKET] tungstenite::connect() returned");

        let (
            mut socket,
            response,
        ) =
            match connection {
                Ok(result) =>
                    result,

                Err(err) => {
                    send_status(
                        &ui_tx,

                        PrinterStatus::Disconnected,

                        format!(
                            "Socket connection failed: {err}"
                        ),
                    );

                    thread::sleep(
                        RECONNECT_DELAY,
                    );

                    continue;
                }
            };

        log_info(&format!(
            "[SOCKET] HTTP response: {}",
            response.status(),
        ));

        send_status(
            &ui_tx,

            PrinterStatus::Connected,

            "WebSocket connected. Starting Socket.IO session...",
        );

        // ---------------------------------------------
        // READ LOOP
        // ---------------------------------------------

        loop {
            let message =
                match socket.read() {
                    Ok(message) =>
                        message,

                    Err(err) => {
                        log_error(&format!(
                            "[SOCKET ERROR] {}",
                            err,
                        ));

                        send_status(
                            &ui_tx,

                            PrinterStatus::Disconnected,

                            format!(
                                "Socket disconnected: {err}"
                            ),
                        );

                        break;
                    }
                };

            match message {
                Message::Text(text) => {
                    let text =
                        text.to_string();

                    println!(
                        "[SOCKET RX] {}",
                        text,
                    );

                    if let Err(err) =
                        handle_socket_packet(
                            &text,

                            &mut socket,

                            &config,

                            &job_tx,

                            &ui_tx,
                        )
                    {
                        log_error(&format!(
                            "[SOCKET PACKET ERROR] {err:#}"
                        ));
                    }
                }

                Message::Ping(data) => {
                    println!(
                        "[WS] Ping"
                    );

                    if let Err(err) =
                        socket.send(
                            Message::Pong(
                                data,
                            ),
                        )
                    {
                        eprintln!(
                            "[WS] Pong error: {}",
                            err,
                        );
                    }
                }

                Message::Pong(_) => {}

                Message::Close(frame) => {
                    println!(
                        "[SOCKET] Closed: {:?}",
                        frame,
                    );

                    send_status(
                        &ui_tx,

                        PrinterStatus::Disconnected,

                        "Socket.IO server closed connection.",
                    );

                    break;
                }

                _ => {}
            }
        }

        println!(
            "[SOCKET] Reconnecting in {} seconds...",
            RECONNECT_DELAY.as_secs(),
        );

        thread::sleep(
            RECONNECT_DELAY,
        );
    }
}

// =========================================================
// HANDLE ENGINE.IO / SOCKET.IO PACKETS
// =========================================================

fn handle_socket_packet<S>(
    packet: &str,

    socket: &mut tungstenite::WebSocket<S>,

    config: &Config,

    job_tx: &Sender<PrintJob>,

    ui_tx: &Sender<WorkerEvent>,
) -> Result<()>
where
    S:
    std::io::Read
    + std::io::Write,
{
    // =====================================================
    // ENGINE.IO OPEN
    //
    // Example:
    //
    // 0{"sid":"abc","upgrades":[],"pingInterval":25000,...}
    // =====================================================

    if packet.starts_with(
        '0',
    ) {
        log_info("[ENGINE.IO] Session opened");

        // Root Socket.IO namespace connect packet.
        //
        // 4 = Engine.IO message
        // 0 = Socket.IO connect

        socket
            .send(
                Message::Text(
                    "40"
                        .into(),
                ),
            )
            .context(
                "Unable to send Socket.IO connect packet",
            )?;

        log_info("[SOCKET TX] 40");

        return Ok(());
    }

    // =====================================================
    // ENGINE.IO PING
    //
    // Engine.IO v3:
    //
    // server -> client
    // 2
    //
    // client -> server
    // 3
    // =====================================================

    if packet == "2" {
        println!(
            "[ENGINE.IO] Ping -> Pong"
        );

        socket
            .send(
                Message::Text(
                    "3"
                        .into(),
                ),
            )
            .context(
                "Unable to send Engine.IO pong",
            )?;

        return Ok(());
    }

    // Ping may theoretically contain data.
    if packet.starts_with(
        '2',
    ) {
        let data =
            &packet[1..];

        let pong =
            format!(
                "3{}",
                data,
            );

        socket
            .send(
                Message::Text(
                    pong.into(),
                ),
            )?;

        return Ok(());
    }

    // =====================================================
    // SOCKET.IO CONNECTED
    //
    // 40
    // =====================================================

    if packet == "40" {
        log_info("[SOCKET.IO] Root namespace connected");

        send_status(
            ui_tx,

            PrinterStatus::Waiting,

            "Connected. Waiting for chat message...",
        );

        return Ok(());
    }

    // =====================================================
    // SOCKET.IO EVENT
    //
    // 42["chat message","https://..."]
    //
    // 4 = Engine.IO message
    // 2 = Socket.IO event
    // =====================================================

    if let Some(
        json_payload,
    ) =
        packet.strip_prefix(
            "42",
        )
    {
        process_socket_event(
            json_payload,

            config,

            job_tx,

            ui_tx,
        )?;

        return Ok(());
    }

    // =====================================================
    // ENGINE.IO CLOSE
    // =====================================================

    if packet == "1" {
        bail!(
            "Engine.IO server requested disconnect"
        );
    }

    println!(
        "[SOCKET] Unhandled packet: {}",
        packet,
    );

    Ok(())
}

// =========================================================
// PROCESS SOCKET.IO EVENT
// =========================================================

fn process_socket_event(
    json_payload: &str,

    config: &Config,

    job_tx: &Sender<PrintJob>,

    ui_tx: &Sender<WorkerEvent>,
) -> Result<()> {
    let data:
        Value =
        serde_json::from_str(
            json_payload,
        )
            .context(
                "Invalid Socket.IO event JSON",
            )?;

    let array =
        data
            .as_array()
            .context(
                "Socket.IO event payload is not an array",
            )?;

    if array.len() <
        2
    {
        bail!(
            "Socket.IO event payload has fewer than 2 elements"
        );
    }

    // ---------------------------------------------
    // Event name
    // ---------------------------------------------

    let event_name =
        array[0]
            .as_str()
            .context(
                "Socket.IO event name is not a string",
            )?;

    println!(
        "[SOCKET EVENT] {}",
        event_name,
    );

    // Ignore unrelated events.
    if event_name
        != config
        .socket_print_event
    {
        println!(
            "[SOCKET] Ignoring event: {}",
            event_name,
        );

        return Ok(());
    }

    // ---------------------------------------------
    // Payload
    // ---------------------------------------------

    let encoded_url =
        array[1]
            .as_str()
            .context(
                "chat message payload is not a string",
            )?;

    println!(
        "[PRINT JOB] Raw URL: {}",
        encoded_url,
    );

    // The JavaScript client calls:
    //
    // encodeURI(reportUrl)
    //
    // Decode percent encoding.
    let decoded_url =
        urlencoding::decode(
            encoded_url,
        )
            .context(
                "Unable to decode report URL",
            )?
            .into_owned();

    println!(
        "[PRINT JOB] URL: {}",
        decoded_url,
    );

    validate_pdf_url(
        &decoded_url,
    )?;

    let _ =
        ui_tx.send(
            WorkerEvent::Url(
                decoded_url
                    .clone(),
            ),
        );

    send_status(
        ui_tx,

        PrinterStatus::Waiting,

        "Print job received. Queued...",
    );

    job_tx
        .send(
            PrintJob {
                url:
                decoded_url,
            },
        )
        .context(
            "Print worker is unavailable",
        )?;

    Ok(())
}

// =========================================================
// VALIDATE PDF URL
// =========================================================

fn validate_pdf_url(
    url_string: &str,
) -> Result<()> {
    let url =
        Url::parse(
            url_string,
        )
            .context(
                "Invalid report URL",
            )?;

    match url.scheme() {
        "http"
        | "https" => {}

        scheme => {
            bail!(
                "Unsupported PDF URL scheme: {}",
                scheme,
            );
        }
    }

    Ok(())
}

// =========================================================
// PRINT WORKER
// =========================================================

fn print_worker(
    job_rx: Receiver<PrintJob>,

    http_client: Client,

    ui_tx: Sender<WorkerEvent>,
) {
    for job in job_rx {
        println!(
            "[PRINT] Processing {}",
            job.url,
        );

        let result =
            process_print_job(
                &http_client,

                &job.url,

                &ui_tx,
            );

        match result {
            Ok(_) => {
                send_status(
                    &ui_tx,

                    PrinterStatus::Success,

                    "PDF printed successfully.",
                );

                println!(
                    "[SUCCESS] PDF printed successfully"
                );

                // After a few seconds show waiting state again.
                thread::sleep(
                    Duration::from_secs(
                        2,
                    ),
                );

                send_status(
                    &ui_tx,

                    PrinterStatus::Waiting,

                    "Connected. Waiting for chat message...",
                );
            }

            Err(err) => {
                eprintln!(
                    "[PRINT ERROR] {err:#}"
                );

                send_status(
                    &ui_tx,

                    PrinterStatus::Error,

                    format!(
                        "Print failed: {err:#}"
                    ),
                );
            }
        }
    }
}

// =========================================================
// PROCESS PRINT JOB
// =========================================================

fn process_print_job(
    http_client: &Client,

    pdf_url: &str,

    ui_tx: &Sender<WorkerEvent>,
) -> Result<()> {
    // ---------------------------------------------
    // DOWNLOAD
    // ---------------------------------------------

    send_status(
        ui_tx,

        PrinterStatus::Downloading,

        "Downloading PDF...",
    );

    println!(
        "[DOWNLOAD] {}",
        pdf_url,
    );

    let pdf =
        download_pdf(
            http_client,

            pdf_url,
        )?;

    println!(
        "[DOWNLOAD] Saved: {}",
        pdf.path().display(),
    );

    // ---------------------------------------------
    // PRINT
    // ---------------------------------------------

    send_status(
        ui_tx,

        PrinterStatus::Printing,

        "Sending PDF to default printer...",
    );

    // Prepare a temporary receipt PDF that is exactly 76 mm wide,
    // keeps 2.5 mm on the left and right, and preserves variable height.
    let prepared_pdf =
        prepare_receipt_pdf(
            pdf.path(),
        )?;

    println!(
        "[PRINT] Prepared PDF: {}",
        prepared_pdf.file.path().display(),
    );

    println!(
        "[PRINT] Paper: {:.2} mm x {:.2} mm",
        PAPER_WIDTH_MM,
        prepared_pdf.paper_height_mm,
    );

    print_pdf(
        prepared_pdf.file.path(),
        prepared_pdf.paper_height_mm,
    )?;

    // Keep temporary file around briefly for the spooler.
    thread::sleep(
        Duration::from_secs(
            3,
        ),
    );

    Ok(())
}

// =========================================================
// PREPARE RECEIPT PDF
// =========================================================

fn pdf_number_to_f64(
    object: &Object,
) -> Result<f64> {
    match object {
        Object::Integer(value) =>
            Ok(*value as f64),

        Object::Real(value) =>
            Ok(*value as f64),

        _ =>
            bail!(
                "PDF page box contains a non-numeric value"
            ),
    }
}

fn get_inherited_page_value<'a>(
    document: &'a Document,
    start_id: lopdf::ObjectId,
    key: &[u8],
) -> Option<&'a Object> {
    let mut current_id =
        Some(start_id);

    while let Some(object_id) =
        current_id
    {
        let dictionary = document
            .get_object(object_id)
            .ok()?
            .as_dict()
            .ok()?;

        if let Ok(value) =
            dictionary.get(key)
        {
            return Some(value);
        }

        current_id = dictionary
            .get(b"Parent")
            .ok()
            .and_then(
                |object| object
                    .as_reference()
                    .ok(),
            );
    }

    None
}

fn read_page_box(
    document: &Document,
    page_id: lopdf::ObjectId,
) -> Result<(f64, f64, f64, f64)> {
    // CropBox and MediaBox may be inherited from the page tree.
    // Prefer CropBox because it represents the visible page area.
    let page_box_object =
        get_inherited_page_value(
            document,
            page_id,
            b"CropBox",
        )
            .or_else(
                || {
                    get_inherited_page_value(
                        document,
                        page_id,
                        b"MediaBox",
                    )
                },
            )
            .context(
                "PDF page has no CropBox or MediaBox",
            )?;

    let page_box = page_box_object
        .as_array()
        .context("PDF page box is not an array")?;

    if page_box.len() != 4 {
        bail!(
            "PDF page box must contain exactly 4 values"
        );
    }

    let x0 = pdf_number_to_f64(&page_box[0])?;
    let y0 = pdf_number_to_f64(&page_box[1])?;
    let x1 = pdf_number_to_f64(&page_box[2])?;
    let y1 = pdf_number_to_f64(&page_box[3])?;

    Ok((x0, y0, x1, y1))
}

fn append_original_contents(
    document: &mut Document,
    original_contents: Option<Object>,
    new_contents: &mut Vec<Object>,
) -> Result<()> {
    let Some(contents) = original_contents else {
        return Ok(());
    };

    match contents {
        Object::Reference(object_id) => {
            new_contents.push(
                Object::Reference(object_id),
            );
        }

        Object::Array(items) => {
            for item in items {
                match item {
                    Object::Reference(object_id) => {
                        new_contents.push(
                            Object::Reference(object_id),
                        );
                    }

                    Object::Stream(stream) => {
                        let object_id =
                            document.add_object(stream);

                        new_contents.push(
                            Object::Reference(object_id),
                        );
                    }

                    _ => {
                        bail!(
                            "Unsupported PDF Contents array item"
                        );
                    }
                }
            }
        }

        Object::Stream(stream) => {
            let object_id =
                document.add_object(stream);

            new_contents.push(
                Object::Reference(object_id),
            );
        }

        Object::Null => {}

        _ => {
            bail!(
                "Unsupported PDF Contents object"
            );
        }
    }

    Ok(())
}

struct PreparedReceiptPdf {
    file: NamedTempFile,
    paper_height_mm: f64,
}

fn prepare_receipt_pdf(
    source_path: &Path,
) -> Result<PreparedReceiptPdf> {
    let mut document =
        Document::load(source_path)
            .context(
                "Unable to open PDF for receipt margin preparation",
            )?;

    let pages = document.get_pages();

    if pages.is_empty() {
        bail!(
            "PDF contains no pages"
        );
    }

    // Thermal receipt jobs must be one continuous page because the printer
    // needs one exact custom paper height for the job.
    if pages.len() != 1 {
        bail!(
            "Receipt PDF must contain exactly one page; found {} pages",
            pages.len(),
        );
    }

    let paper_width_pt =
        PAPER_WIDTH_MM * 72.0 / 25.4;

    let content_width_pt =
        CONTENT_WIDTH_MM * 72.0 / 25.4;

    let mut prepared_height_mm = 0.0_f64;

    for (_page_number, page_id) in pages {
        let (
            x0,
            y0,
            x1,
            y1,
        ) = read_page_box(
            &document,
            page_id,
        )?;

        let source_width_pt =
            (x1 - x0).abs();

        let source_height_pt =
            (y1 - y0).abs();

        if source_width_pt <= 0.0
            || source_height_pt <= 0.0
        {
            bail!(
                "PDF page has invalid dimensions"
            );
        }

        // Uniformly scale only when necessary. The content is never enlarged.
        // A normal 76 mm-ish receipt is reduced just enough to leave at least
        // 2.5 mm on both sides. Narrower receipts stay at 100% and are centered.
        let scale =
            (content_width_pt
                / source_width_pt)
                .min(1.0);

        let scaled_width_pt =
            source_width_pt
                * scale;

        let output_height_pt =
            source_height_pt
                * scale;

        prepared_height_mm =
            output_height_pt * 25.4 / 72.0;

        let actual_left_margin_pt =
            (paper_width_pt - scaled_width_pt)
                / 2.0;

        let translate_x =
            actual_left_margin_pt
                - (x0 * scale);

        let translate_y =
            -(y0 * scale);

        let prefix = format!(
            "q\n{:.8} 0 0 {:.8} {:.8} {:.8} cm\n",
            scale,
            scale,
            translate_x,
            translate_y,
        );

        let prefix_id =
            document.add_object(
                Stream::new(
                    Dictionary::new(),
                    prefix.into_bytes(),
                ),
            );

        let suffix_id =
            document.add_object(
                Stream::new(
                    Dictionary::new(),
                    b"\nQ\n".to_vec(),
                ),
            );

        let original_contents = {
            let page = document
                .get_object(page_id)
                .context(
                    "Unable to read PDF page contents",
                )?
                .as_dict()
                .context(
                    "PDF page object is not a dictionary",
                )?;

            page.get(b"Contents")
                .ok()
                .cloned()
        };

        let mut new_contents = vec![
            Object::Reference(prefix_id),
        ];

        append_original_contents(
            &mut document,
            original_contents,
            &mut new_contents,
        )?;

        new_contents.push(
            Object::Reference(suffix_id),
        );

        let page = document
            .get_object_mut(page_id)
            .context(
                "Unable to update PDF page",
            )?
            .as_dict_mut()
            .context(
                "PDF page object is not a dictionary",
            )?;

        let new_page_box =
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Real(
                    paper_width_pt as f32,
                ),
                Object::Real(
                    output_height_pt as f32,
                ),
            ]);

        page.set(
            "MediaBox",
            new_page_box.clone(),
        );

        page.set(
            "CropBox",
            new_page_box,
        );

        page.set(
            "Contents",
            Object::Array(new_contents),
        );

        // The content stream now has the final orientation and position.
        // Remove page rotation so the printer does not rotate it again.
        page.set(
            "Rotate",
            Object::Integer(0),
        );

        println!(
            "[PDF PREP] Source: {:.2} mm x {:.2} mm",
            source_width_pt * 25.4 / 72.0,
            source_height_pt * 25.4 / 72.0,
        );

        println!(
            "[PDF PREP] Output: {:.2} mm x {:.2} mm, scale {:.4}",
            PAPER_WIDTH_MM,
            output_height_pt * 25.4 / 72.0,
            scale,
        );
    }

    // Remove unused objects after replacing page content streams.
    document.prune_objects();

    let mut prepared =
        Builder::new()
            .prefix(
                "rbprinter_prepared_",
            )
            .suffix(
                ".pdf",
            )
            .tempfile()
            .context(
                "Unable to create prepared PDF temporary file",
            )?;

    document
        .save_to(
            prepared.as_file_mut(),
        )
        .context(
            "Unable to save prepared receipt PDF",
        )?;

    prepared
        .as_file_mut()
        .flush()
        .context(
            "Unable to flush prepared receipt PDF",
        )?;

    prepared
        .as_file_mut()
        .seek(
            SeekFrom::Start(0),
        )
        .context(
            "Unable to rewind prepared receipt PDF",
        )?;

    Ok(
        PreparedReceiptPdf {
            file: prepared,
            paper_height_mm: prepared_height_mm,
        },
    )
}

// =========================================================
// DOWNLOAD PDF
// =========================================================

fn download_pdf(
    http_client: &Client,

    url_string: &str,
) -> Result<NamedTempFile> {
    let url =
        Url::parse(
            url_string,
        )
            .with_context(
                || {
                    format!(
                        "Invalid PDF URL: {}",
                        url_string,
                    )
                },
            )?;

    match url.scheme() {
        "http"
        | "https" => {}

        scheme => {
            bail!(
                "Unsupported URL protocol: {}",
                scheme,
            );
        }
    }

    // ---------------------------------------------
    // REQUEST
    // ---------------------------------------------

    let response =
        http_client
            .get(
                url,
            )
            .send()
            .context(
                "Unable to download PDF",
            )?
            .error_for_status()
            .context(
                "PDF server returned an HTTP error",
            )?;

    // ---------------------------------------------
    // SIZE
    // ---------------------------------------------

    if let Some(size) =
        response
            .content_length()
    {
        println!(
            "[DOWNLOAD] Content length: {} bytes",
            size,
        );

        if size >
            MAX_PDF_SIZE
        {
            bail!(
                "PDF exceeds maximum size of {} MB",
                MAX_PDF_SIZE
                    / 1024
                    / 1024,
            );
        }
    }

    // ---------------------------------------------
    // TEMP FILE
    // ---------------------------------------------

    let mut temp_file =
        Builder::new()
            .prefix(
                "rbprinter_",
            )
            .suffix(
                ".pdf",
            )
            .tempfile()
            .context(
                "Unable to create temporary PDF file",
            )?;

    // ---------------------------------------------
    // DOWNLOAD
    // ---------------------------------------------

    let mut limited =
        response.take(
            MAX_PDF_SIZE
                + 1,
        );

    let downloaded =
        io::copy(
            &mut limited,

            temp_file
                .as_file_mut(),
        )
            .context(
                "Error writing PDF to temporary file",
            )?;

    println!(
        "[DOWNLOAD] {} bytes downloaded",
        downloaded,
    );

    if downloaded >
        MAX_PDF_SIZE
    {
        bail!(
            "Downloaded PDF exceeds {} MB",
            MAX_PDF_SIZE
                / 1024
                / 1024,
        );
    }

    temp_file
        .as_file_mut()
        .flush()
        .context(
            "Unable to flush downloaded PDF",
        )?;

    // ---------------------------------------------
    // VALIDATE %PDF-
    // ---------------------------------------------

    temp_file
        .as_file_mut()
        .seek(
            SeekFrom::Start(
                0,
            ),
        )?;

    let mut header =
        [0_u8; 5];

    temp_file
        .as_file_mut()
        .read_exact(
            &mut header,
        )
        .context(
            "Downloaded file is too small to be a PDF",
        )?;

    if &header
        != b"%PDF-"
    {
        bail!(
            "Downloaded file is not a valid PDF"
        );
    }

    temp_file
        .as_file_mut()
        .seek(
            SeekFrom::Start(
                0,
            ),
        )?;

    println!(
        "[DOWNLOAD] PDF validation successful"
    );

    Ok(
        temp_file,
    )
}

// =========================================================
// LOGGING / CRASH DIAGNOSTICS
// =========================================================

fn log_file_path(
    file_name: &str,
) -> Option<std::path::PathBuf> {
    let exe_path = env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;

    Some(exe_dir.join(file_name))
}

fn append_to_file(
    file_name: &str,
    message: &str,
) {
    let Some(path) = log_file_path(file_name) else {
        return;
    };

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            file,
            "{:?} {}",
            std::time::SystemTime::now(),
            message,
        );
    }
}

fn log_info(
    message: &str,
) {
    println!("{}", message);
    append_to_file("rbprinter.log", message);
}

fn log_error(
    message: &str,
) {
    eprintln!("{}", message);
    append_to_file("rbprinter.log", message);
}

fn install_crash_logger() {
    panic::set_hook(Box::new(|panic_info| {
        let backtrace =
            std::backtrace::Backtrace::force_capture();

        let message = format!(
            "[PANIC] {}\nBACKTRACE:\n{}",
            panic_info,
            backtrace,
        );

        eprintln!("{}", message);

        append_to_file(
            "rbprinter-crash.log",
            &message,
        );

        append_to_file(
            "rbprinter.log",
            &message,
        );
    }));
}

// =========================================================
// STATUS
// =========================================================

fn send_status(
    tx: &Sender<WorkerEvent>,
    status: PrinterStatus,
    message: impl Into<String>,
) {
    let message = message.into();

    let prefix = match &status {
        PrinterStatus::Error => "[ERROR]",
        PrinterStatus::Disconnected => "[DISCONNECTED]",
        PrinterStatus::Connected => "[CONNECTED]",
        PrinterStatus::Waiting => "[WAITING]",
        PrinterStatus::Connecting => "[CONNECTING]",
        PrinterStatus::Downloading => "[DOWNLOAD]",
        PrinterStatus::Printing => "[PRINT]",
        PrinterStatus::Success => "[SUCCESS]",
        PrinterStatus::Starting => "[STARTING]",
    };

    let log_message = format!(
        "{} {}",
        prefix,
        message,
    );

    match &status {
        PrinterStatus::Error
        | PrinterStatus::Disconnected => {
            log_error(&log_message);
        }

        _ => {
            log_info(&log_message);
        }
    }

    let _ = tx.send(
        WorkerEvent::Status(status),
    );

    let _ = tx.send(
        WorkerEvent::Message(message),
    );
}

// =========================================================
// WINDOWS PRINT
// =========================================================

#[cfg(target_os = "windows")]
fn print_pdf(
    path: &Path,
    paper_height_mm: f64,
) -> Result<()> {
    let exe_path = env::current_exe()
        .context("Unable to determine application path")?;

    let exe_dir = exe_path
        .parent()
        .context("Unable to determine application directory")?;

    let sumatra = exe_dir.join("SumatraPDF.exe");

    if !sumatra.exists() {
        bail!(
            "SumatraPDF.exe not found at {}",
            sumatra.display()
        );
    }

    println!("[PRINT] PDF: {}", path.display());
    println!("[PRINT] Sending to Windows default printer");

    // The prepared PDF already contains the 2.5 mm margins.
    // Pass the exact 76 mm x dynamic-height paper size to Sumatra so the
    // thermal driver does not fall back to a fixed/long page size.
    let print_settings = format!(
        "noscale,center,paper={:.2}mm x {:.2}mm,disable-auto-rotation,ignore-pdf-print-settings",
        PAPER_WIDTH_MM,
        paper_height_mm,
    );

    println!(
        "[PRINT] Settings: {}",
        print_settings,
    );

    let status = Command::new(&sumatra)
        .arg("-print-to-default")
        .arg("-print-settings")
        .arg(&print_settings)
        .arg("-silent")
        .arg(path)
        .status()
        .context("Unable to start SumatraPDF")?;

    if !status.success() {
        bail!(
            "Printing failed. SumatraPDF exit code: {:?}",
            status.code()
        );
    }

    println!("[PRINT] PDF submitted to default printer");

    Ok(())
}

// =========================================================
// LINUX PRINT
// =========================================================

#[cfg(target_os = "linux")]
fn print_pdf(
    path: &Path,
    _paper_height_mm: f64,
) -> Result<()> {
    let status =
        Command::new(
            "lp",
        )
            .arg(
                path,
            )
            .status()
            .context(
                "Unable to execute lp",
            )?;

    if !status.success() {
        bail!(
            "lp print command failed"
        );
    }

    Ok(())
}

// =========================================================
// MACOS PRINT
// =========================================================

#[cfg(target_os = "macos")]
fn print_pdf(
    path: &Path,
    _paper_height_mm: f64,
) -> Result<()> {
    let status =
        Command::new(
            "lp",
        )
            .arg(
                path,
            )
            .status()
            .context(
                "Unable to execute lp",
            )?;

    if !status.success() {
        bail!(
            "macOS print command failed"
        );
    }

    Ok(())
}

// =========================================================
// UNSUPPORTED PLATFORM
// =========================================================

#[cfg(not(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "macos"
)))]
fn print_pdf(
    _path: &Path,
    _paper_height_mm: f64,
) -> Result<()> {
    bail!(
        "Unsupported operating system"
    );
}
