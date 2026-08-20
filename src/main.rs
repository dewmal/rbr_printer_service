#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use anyhow::{bail, Context, Result};
use eframe::egui;
use reqwest::blocking::Client;
use serde_json::Value;

use std::{
    env,
    io::{self, Read, Seek, SeekFrom, Write},
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
    let (
        ui_tx,
        ui_rx,
    ) =
        mpsc::channel::<WorkerEvent>();

    thread::spawn(
        move || {
            service_worker(
                ui_tx,
            );
        },
    );

    let options =
        eframe::NativeOptions {
            viewport:
            egui::ViewportBuilder::default()
                .with_title(
                    "PDF Printer Client",
                )
                .with_inner_size(
                    [
                        500.0,
                        450.0,
                    ],
                )
                .with_min_inner_size(
                    [
                        500.0,
                        450.0,
                    ],
                )
                .with_resizable(
                    false,
                ),

            ..Default::default()
        };

    eframe::run_native(
        "PDF Printer Client",

        options,

        Box::new(
            move |_cc| {
                Ok(
                    Box::new(
                        PrinterApp::new(
                            ui_rx,
                        ),
                    ),
                )
            },
        ),
    )
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

    thread::spawn(
        move || {
            print_worker(
                job_rx,

                http,

                print_ui_tx,
            );
        },
    );

    // ---------------------------------------------
    // SOCKET.IO
    // ---------------------------------------------

    socket_worker(
        config,

        job_tx,

        ui_tx,
    );
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

    println!(
        "[CONFIG] .env = {}",
        env_path.display(),
    );

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

        println!(
            "[SOCKET] Connecting: {}",
            websocket_url,
        );

        // ---------------------------------------------
        // CONNECT
        // ---------------------------------------------

        let connection =
            connect(
                websocket_url
                    .as_str(),
            );

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

        println!(
            "[SOCKET] HTTP response: {}",
            response.status(),
        );

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
                        eprintln!(
                            "[SOCKET ERROR] {}",
                            err,
                        );

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
                        eprintln!(
                            "[SOCKET PACKET ERROR] {err:#}"
                        );
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
        println!(
            "[ENGINE.IO] Session opened"
        );

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

        println!(
            "[SOCKET TX] 40"
        );

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
        println!(
            "[SOCKET.IO] Root namespace connected"
        );

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

    print_pdf(
        pdf.path(),
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
// STATUS
// =========================================================

fn send_status(
    tx: &Sender<WorkerEvent>,

    status: PrinterStatus,

    message: impl Into<String>,
) {
    let message =
        message.into();

    match &status {
        PrinterStatus::Error => {
            eprintln!(
                "[ERROR] {}",
                message,
            );
        }

        PrinterStatus::Disconnected => {
            eprintln!(
                "[DISCONNECTED] {}",
                message,
            );
        }

        PrinterStatus::Connected => {
            println!(
                "[CONNECTED] {}",
                message,
            );
        }

        PrinterStatus::Waiting => {
            println!(
                "[WAITING] {}",
                message,
            );
        }

        PrinterStatus::Connecting => {
            println!(
                "[CONNECTING] {}",
                message,
            );
        }

        PrinterStatus::Downloading => {
            println!(
                "[DOWNLOAD] {}",
                message,
            );
        }

        PrinterStatus::Printing => {
            println!(
                "[PRINT] {}",
                message,
            );
        }

        PrinterStatus::Success => {
            println!(
                "[SUCCESS] {}",
                message,
            );
        }

        PrinterStatus::Starting => {
            println!(
                "[STARTING] {}",
                message,
            );
        }
    }

    let _ =
        tx.send(
            WorkerEvent::Status(
                status,
            ),
        );

    let _ =
        tx.send(
            WorkerEvent::Message(
                message,
            ),
        );
}

// =========================================================
// WINDOWS PRINT
// =========================================================

#[cfg(target_os = "windows")]
fn print_pdf(
    path: &Path,
) -> Result<()> {
    println!(
        "[PRINT] PDF file: {}",
        path.display(),
    );

    // ---------------------------------------------
    // SumatraPDF next to executable
    // ---------------------------------------------

    if let Ok(exe_path) =
        env::current_exe()
    {
        if let Some(directory) =
            exe_path.parent()
        {
            let sumatra =
                directory.join(
                    "SumatraPDF.exe",
                );

            if sumatra.exists() {
                println!(
                    "[PRINT] Using SumatraPDF: {}",
                    sumatra.display(),
                );

                let status =
                    Command::new(
                        &sumatra,
                    )
                        .arg(
                            "-print-to-default",
                        )
                        .arg(
                            "-silent",
                        )
                        .arg(
                            path,
                        )
                        .status()
                        .with_context(
                            || {
                                format!(
                                    "Unable to start {}",
                                    sumatra.display(),
                                )
                            },
                        )?;

                println!(
                    "[PRINT] Sumatra exit code: {:?}",
                    status.code(),
                );

                if status.success() {
                    return Ok(());
                }

                bail!(
                    "SumatraPDF printing failed with exit code {:?}",
                    status.code(),
                );
            }
        }
    }

    // ---------------------------------------------
    // SumatraPDF from PATH
    // ---------------------------------------------

    println!(
        "[PRINT] Checking SumatraPDF in PATH..."
    );

    match Command::new(
        "SumatraPDF.exe",
    )
        .arg(
            "-print-to-default",
        )
        .arg(
            "-silent",
        )
        .arg(
            path,
        )
        .status()
    {
        Ok(status)
        if status.success() =>
            {
                return Ok(());
            }

        Ok(status) => {
            eprintln!(
                "[PRINT] Sumatra in PATH failed: {:?}",
                status.code(),
            );
        }

        Err(err)
        if err.kind()
            == io::ErrorKind::NotFound =>
            {
                println!(
                    "[PRINT] SumatraPDF not found"
                );
            }

        Err(err) => {
            return Err(
                err.into(),
            );
        }
    }

    // ---------------------------------------------
    // Windows Print verb fallback
    // ---------------------------------------------

    println!(
        "[PRINT] Using Windows Print shell verb"
    );

    let file =
        path
            .to_string_lossy()
            .replace(
                '\'',
                "''",
            );

    let command =
        format!(
            "Start-Process -FilePath '{}' -Verb Print -Wait",
            file,
        );

    let status =
        Command::new(
            "powershell.exe",
        )
            .arg(
                "-NoProfile",
            )
            .arg(
                "-NonInteractive",
            )
            .arg(
                "-Command",
            )
            .arg(
                command,
            )
            .status()
            .context(
                "Unable to invoke Windows PDF print command",
            )?;

    if !status.success() {
        bail!(
            "Windows PDF printing failed"
        );
    }

    Ok(())
}

// =========================================================
// LINUX PRINT
// =========================================================

#[cfg(target_os = "linux")]
fn print_pdf(
    path: &Path,
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
) -> Result<()> {
    bail!(
        "Unsupported operating system"
    );
}