use crate::*;
use crate::{pdf::*, printer::*};

// =========================================================
// SERVICE WORKER
// =========================================================

pub(crate) fn service_worker(
    ui_tx: Sender<WorkerEvent>,
    print_settings: Arc<RwLock<PrintSettings>>,
) {
    send_status(&ui_tx, PrinterStatus::Starting, "Loading configuration...");

    // ---------------------------------------------
    // CONFIG
    // ---------------------------------------------

    let config = match load_config() {
        Ok(config) => config,

        Err(err) => {
            send_status(
                &ui_tx,
                PrinterStatus::Error,
                format!("Configuration error: {err:#}"),
            );

            return;
        }
    };

    let _ = ui_tx.send(WorkerEvent::Server(config.socket_server.clone()));

    let _ = ui_tx.send(WorkerEvent::SocketEvent(config.socket_print_event.clone()));

    if let Ok(mut current) = print_settings.write() {
        *current = config.print_settings;
    }

    let _ = ui_tx.send(WorkerEvent::PrintSettingsLoaded {
        settings: config.print_settings,
        env_path: config.env_path.clone(),
    });

    // ---------------------------------------------
    // HTTP CLIENT
    // ---------------------------------------------

    let http = match Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent("rbprinter/1.0")
        .build()
    {
        Ok(client) => client,

        Err(err) => {
            send_status(
                &ui_tx,
                PrinterStatus::Error,
                format!("HTTP client error: {err}"),
            );

            return;
        }
    };

    // ---------------------------------------------
    // PRINT JOB QUEUE
    // ---------------------------------------------

    let (job_tx, job_rx) = mpsc::channel::<PrintJob>();

    let print_ui_tx = ui_tx.clone();

    let print_panic_ui_tx = print_ui_tx.clone();

    let print_worker_settings = print_settings.clone();

    thread::spawn(move || {
        let print_result = panic::catch_unwind(AssertUnwindSafe(|| {
            print_worker(job_rx, http, print_ui_tx, print_worker_settings);
        }));

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

    let socket_panic_ui_tx = ui_tx.clone();

    let socket_result = panic::catch_unwind(AssertUnwindSafe(|| {
        socket_worker(config, job_tx, socket_panic_ui_tx.clone());
    }));

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
// SOCKET.IO 1.x / ENGINE.IO 3 WEBSOCKET URL
// =========================================================

pub(crate) fn build_socket_url(server_url: &str) -> Result<String> {
    let mut url = Url::parse(server_url)?;

    match url.scheme() {
        "http" => {
            url.set_scheme("ws")
                .map_err(|_| anyhow::anyhow!("Unable to convert http to ws"))?;
        }

        "https" => {
            url.set_scheme("wss")
                .map_err(|_| anyhow::anyhow!("Unable to convert https to wss"))?;
        }

        "ws" | "wss" => {}

        scheme => {
            bail!("Unsupported scheme: {scheme}");
        }
    }

    // Socket.IO default path.
    url.set_path("/socket.io/");

    url.set_query(Some("EIO=3&transport=websocket"));

    Ok(url.to_string())
}

// =========================================================
// SOCKET WORKER
// =========================================================

pub(crate) fn socket_worker(config: Config, job_tx: Sender<PrintJob>, ui_tx: Sender<WorkerEvent>) {
    loop {
        send_status(
            &ui_tx,
            PrinterStatus::Connecting,
            format!("Connecting to {}...", config.socket_server,),
        );

        let websocket_url = match build_socket_url(&config.socket_server) {
            Ok(url) => url,

            Err(err) => {
                send_status(
                    &ui_tx,
                    PrinterStatus::Error,
                    format!("Unable to create WebSocket URL: {err:#}"),
                );

                thread::sleep(RECONNECT_DELAY);

                continue;
            }
        };

        log_info(&format!("[SOCKET] Connecting: {}", websocket_url,));

        // ---------------------------------------------
        // CONNECT
        // ---------------------------------------------

        log_info("[SOCKET] calling tungstenite::connect()");

        let connection = connect(websocket_url.as_str());

        log_info("[SOCKET] tungstenite::connect() returned");

        let (mut socket, response) = match connection {
            Ok(result) => result,

            Err(err) => {
                send_status(
                    &ui_tx,
                    PrinterStatus::Disconnected,
                    format!("Socket connection failed: {err}"),
                );

                thread::sleep(RECONNECT_DELAY);

                continue;
            }
        };

        log_info(&format!("[SOCKET] HTTP response: {}", response.status(),));

        send_status(
            &ui_tx,
            PrinterStatus::Connected,
            "WebSocket connected. Starting Socket.IO session...",
        );

        // ---------------------------------------------
        // READ LOOP
        // ---------------------------------------------

        loop {
            let message = match socket.read() {
                Ok(message) => message,

                Err(err) => {
                    log_error(&format!("[SOCKET ERROR] {}", err,));

                    send_status(
                        &ui_tx,
                        PrinterStatus::Disconnected,
                        format!("Socket disconnected: {err}"),
                    );

                    break;
                }
            };

            match message {
                Message::Text(text) => {
                    let text = text.to_string();

                    println!("[SOCKET RX] {}", text,);

                    if let Err(err) =
                        handle_socket_packet(&text, &mut socket, &config, &job_tx, &ui_tx)
                    {
                        log_error(&format!("[SOCKET PACKET ERROR] {err:#}"));
                    }
                }

                Message::Ping(data) => {
                    println!("[WS] Ping");

                    if let Err(err) = socket.send(Message::Pong(data)) {
                        eprintln!("[WS] Pong error: {}", err,);
                    }
                }

                Message::Pong(_) => {}

                Message::Close(frame) => {
                    println!("[SOCKET] Closed: {:?}", frame,);

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

        thread::sleep(RECONNECT_DELAY);
    }
}

// =========================================================
// HANDLE ENGINE.IO / SOCKET.IO PACKETS
// =========================================================

pub(crate) fn handle_socket_packet<S>(
    packet: &str,

    socket: &mut tungstenite::WebSocket<S>,

    config: &Config,

    job_tx: &Sender<PrintJob>,

    ui_tx: &Sender<WorkerEvent>,
) -> Result<()>
where
    S: std::io::Read + std::io::Write,
{
    // =====================================================
    // ENGINE.IO OPEN
    //
    // Example:
    //
    // 0{"sid":"abc","upgrades":[],"pingInterval":25000,...}
    // =====================================================

    if packet.starts_with('0') {
        log_info("[ENGINE.IO] Session opened");

        // Root Socket.IO namespace connect packet.
        //
        // 4 = Engine.IO message
        // 0 = Socket.IO connect

        socket
            .send(Message::Text("40".into()))
            .context("Unable to send Socket.IO connect packet")?;

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
        println!("[ENGINE.IO] Ping -> Pong");

        socket
            .send(Message::Text("3".into()))
            .context("Unable to send Engine.IO pong")?;

        return Ok(());
    }

    // Ping may theoretically contain data.
    if packet.starts_with('2') {
        let data = &packet[1..];

        let pong = format!("3{}", data,);

        socket.send(Message::Text(pong.into()))?;

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

    if let Some(json_payload) = packet.strip_prefix("42") {
        process_socket_event(json_payload, config, job_tx, ui_tx)?;

        return Ok(());
    }

    // =====================================================
    // ENGINE.IO CLOSE
    // =====================================================

    if packet == "1" {
        bail!("Engine.IO server requested disconnect");
    }

    println!("[SOCKET] Unhandled packet: {}", packet,);

    Ok(())
}

// =========================================================
// PROCESS SOCKET.IO EVENT
// =========================================================

pub(crate) fn process_socket_event(
    json_payload: &str,

    config: &Config,

    job_tx: &Sender<PrintJob>,

    ui_tx: &Sender<WorkerEvent>,
) -> Result<()> {
    let data: Value = serde_json::from_str(json_payload).context("Invalid Socket.IO event JSON")?;

    let array = data
        .as_array()
        .context("Socket.IO event payload is not an array")?;

    if array.len() < 2 {
        bail!("Socket.IO event payload has fewer than 2 elements");
    }

    // ---------------------------------------------
    // Event name
    // ---------------------------------------------

    let event_name = array[0]
        .as_str()
        .context("Socket.IO event name is not a string")?;

    println!("[SOCKET EVENT] {}", event_name,);

    // Ignore unrelated events.
    if event_name != config.socket_print_event {
        println!("[SOCKET] Ignoring event: {}", event_name,);

        return Ok(());
    }

    // ---------------------------------------------
    // Payload
    // ---------------------------------------------

    let encoded_url = array[1]
        .as_str()
        .context("chat message payload is not a string")?;

    println!("[PRINT JOB] Raw URL: {}", encoded_url,);

    // The JavaScript client calls:
    //
    // encodeURI(reportUrl)
    //
    // Decode percent encoding.
    let decoded_url = urlencoding::decode(encoded_url)
        .context("Unable to decode report URL")?
        .into_owned();

    println!("[PRINT JOB] URL: {}", decoded_url,);

    validate_pdf_url(&decoded_url)?;

    let _ = ui_tx.send(WorkerEvent::Url(decoded_url.clone()));

    send_status(
        ui_tx,
        PrinterStatus::Waiting,
        "Print job received. Queued...",
    );

    job_tx
        .send(PrintJob { url: decoded_url })
        .context("Print worker is unavailable")?;

    Ok(())
}

// =========================================================
// VALIDATE PDF URL
// =========================================================

pub(crate) fn validate_pdf_url(url_string: &str) -> Result<()> {
    let url = Url::parse(url_string).context("Invalid report URL")?;

    match url.scheme() {
        "http" | "https" => {}

        scheme => {
            bail!("Unsupported PDF URL scheme: {}", scheme,);
        }
    }

    Ok(())
}

// =========================================================
// PRINT WORKER
// =========================================================

pub(crate) fn print_worker(
    job_rx: Receiver<PrintJob>,

    http_client: Client,

    ui_tx: Sender<WorkerEvent>,

    print_settings: Arc<RwLock<PrintSettings>>,
) {
    for job in job_rx {
        println!("[PRINT] Processing {}", job.url,);

        let settings = print_settings
            .read()
            .map(|settings| *settings)
            .unwrap_or_default();

        let result = process_print_job(&http_client, &job.url, &ui_tx, settings);

        match result {
            Ok(_) => {
                send_status(&ui_tx, PrinterStatus::Success, "PDF printed successfully.");

                println!("[SUCCESS] PDF printed successfully");

                // After a few seconds show waiting state again.
                thread::sleep(Duration::from_secs(2));

                send_status(
                    &ui_tx,
                    PrinterStatus::Waiting,
                    "Connected. Waiting for chat message...",
                );
            }

            Err(err) => {
                eprintln!("[PRINT ERROR] {err:#}");

                send_status(
                    &ui_tx,
                    PrinterStatus::Error,
                    format!("Print failed: {err:#}"),
                );
            }
        }
    }
}

// =========================================================
// PROCESS PRINT JOB
// =========================================================

pub(crate) fn process_print_job(
    http_client: &Client,

    pdf_url: &str,

    ui_tx: &Sender<WorkerEvent>,

    print_settings: PrintSettings,
) -> Result<()> {
    // ---------------------------------------------
    // DOWNLOAD
    // ---------------------------------------------

    send_status(ui_tx, PrinterStatus::Downloading, "Downloading PDF...");

    println!("[DOWNLOAD] {}", pdf_url,);

    let pdf = download_pdf(http_client, pdf_url)?;

    println!("[DOWNLOAD] Saved: {}", pdf.path().display(),);

    // ---------------------------------------------
    // PRINT
    // ---------------------------------------------

    send_status(
        ui_tx,
        PrinterStatus::Printing,
        "Sending PDF to default printer...",
    );

    // Prepare a temporary receipt PDF using the current UI settings.
    // Width/margins are configurable; height remains dynamic per PDF.
    let prepared_pdf = prepare_receipt_pdf(pdf.path(), print_settings)?;

    println!(
        "[PRINT] Prepared PDF: {}",
        prepared_pdf.file.path().display(),
    );

    println!(
        "[PRINT] Paper: {:.2} mm x {:.2} mm",
        print_settings.paper_width_mm, prepared_pdf.paper_height_mm,
    );

    if print_settings.preview_before_print {
        send_status(
            ui_tx,
            PrinterStatus::Printing,
            "Preview open. Close SumatraPDF to continue printing...",
        );

        preview_pdf(prepared_pdf.file.path())?;

        send_status(
            ui_tx,
            PrinterStatus::Printing,
            "Preview closed. Sending PDF to default printer...",
        );
    }

    print_pdf(
        prepared_pdf.file.path(),
        prepared_pdf.paper_height_mm,
        print_settings.paper_width_mm,
    )?;

    // Keep temporary file around briefly for the spooler.
    thread::sleep(Duration::from_secs(3));

    Ok(())
}
