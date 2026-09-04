use crate::*;

// =========================================================
// GUI STATUS
// =========================================================

#[derive(Clone, Debug)]
pub(crate) enum PrinterStatus {
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

pub(crate) enum WorkerEvent {
    Status(PrinterStatus),
    Server(String),
    SocketEvent(String),
    Url(String),
    Message(String),
    PrintSettingsLoaded {
        settings: PrintSettings,
        env_path: PathBuf,
    },
}

// =========================================================
// PRINT JOB
// =========================================================

pub(crate) struct PrintJob {
    pub(crate) url: String,
}

// =========================================================
// GUI
// =========================================================

pub(crate) struct PrinterApp {
    status: PrinterStatus,

    server: String,

    socket_event: String,

    last_url: String,

    message: String,

    paper_width_mm: f64,

    side_margin_mm: f64,

    env_path: Option<PathBuf>,

    settings_message: String,

    print_settings: Arc<RwLock<PrintSettings>>,

    rx: Receiver<WorkerEvent>,
}

// =========================================================
// GUI IMPLEMENTATION
// =========================================================

impl PrinterApp {
    pub(crate) fn new(
        rx: Receiver<WorkerEvent>,
        print_settings: Arc<RwLock<PrintSettings>>,
    ) -> Self {
        let initial_settings = print_settings
            .read()
            .map(|settings| *settings)
            .unwrap_or_default();

        Self {
            status: PrinterStatus::Starting,

            server: String::new(),

            socket_event: String::new(),

            last_url: String::new(),

            message: "Starting application...".to_string(),

            paper_width_mm: initial_settings.paper_width_mm,

            side_margin_mm: initial_settings.side_margin_mm,

            env_path: None,

            settings_message: String::new(),

            print_settings,

            rx,
        }
    }

    fn process_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                WorkerEvent::Status(status) => {
                    self.status = status;
                }

                WorkerEvent::Server(server) => {
                    self.server = server;
                }

                WorkerEvent::SocketEvent(event) => {
                    self.socket_event = event;
                }

                WorkerEvent::Url(url) => {
                    self.last_url = url;
                }

                WorkerEvent::Message(message) => {
                    self.message = message;
                }

                WorkerEvent::PrintSettingsLoaded { settings, env_path } => {
                    self.paper_width_mm = settings.paper_width_mm;

                    self.side_margin_mm = settings.side_margin_mm;

                    self.env_path = Some(env_path);

                    if let Ok(mut current) = self.print_settings.write() {
                        *current = settings;
                    }
                }
            }
        }
    }

    fn status_text(&self) -> &'static str {
        match self.status {
            PrinterStatus::Starting => "Starting",

            PrinterStatus::Connecting => "Connecting",

            PrinterStatus::Connected => "Connected",

            PrinterStatus::Waiting => "Waiting",

            PrinterStatus::Downloading => "Downloading",

            PrinterStatus::Printing => "Printing",

            PrinterStatus::Success => "Printed",

            PrinterStatus::Error => "Error",

            PrinterStatus::Disconnected => "Disconnected",
        }
    }

    fn status_color(&self) -> egui::Color32 {
        match self.status {
            PrinterStatus::Connected | PrinterStatus::Waiting | PrinterStatus::Success => {
                egui::Color32::from_rgb(40, 180, 90)
            }

            PrinterStatus::Starting
            | PrinterStatus::Connecting
            | PrinterStatus::Downloading
            | PrinterStatus::Printing => egui::Color32::from_rgb(230, 170, 40),

            PrinterStatus::Error | PrinterStatus::Disconnected => {
                egui::Color32::from_rgb(220, 60, 60)
            }
        }
    }
}

// =========================================================
// EFRAME APP
// =========================================================

impl eframe::App for PrinterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.process_events();

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(10.0);

            ui.heading("PDF Printer Client");

            ui.add_space(15.0);

            // -----------------------------
            // STATUS
            // -----------------------------

            ui.horizontal(|ui| {
                ui.colored_label(self.status_color(), egui::RichText::new("â—").size(22.0));

                ui.label(egui::RichText::new(self.status_text()).strong().size(18.0));
            });

            ui.add_space(15.0);

            ui.separator();

            ui.add_space(15.0);

            // -----------------------------
            // SERVER
            // -----------------------------

            ui.label(egui::RichText::new("Socket.IO Server").strong());

            if self.server.is_empty() {
                ui.label("-");
            } else {
                ui.monospace(&self.server);
            }

            ui.add_space(15.0);

            // -----------------------------
            // EVENT
            // -----------------------------

            ui.label(egui::RichText::new("Listening Event").strong());

            if self.socket_event.is_empty() {
                ui.label("-");
            } else {
                ui.monospace(&self.socket_event);
            }

            ui.add_space(15.0);

            // -----------------------------
            // STATUS MESSAGE
            // -----------------------------

            ui.label(egui::RichText::new("Status").strong());

            ui.label(&self.message);

            ui.add_space(15.0);

            // -----------------------------
            // LAST PDF
            // -----------------------------

            ui.label(egui::RichText::new("Last PDF URL").strong());

            if self.last_url.is_empty() {
                ui.label("No print jobs received yet.");
            } else {
                ui.add(
                    egui::Label::new(egui::RichText::new(&self.last_url).monospace().small())
                        .wrap(),
                );
            }

            ui.add_space(15.0);

            ui.separator();

            ui.add_space(12.0);

            // -----------------------------
            // PRINT LAYOUT SETTINGS
            // -----------------------------

            ui.label(egui::RichText::new("Print Layout Settings").strong());

            ui.add_space(6.0);

            egui::Grid::new("print_layout_settings_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Paper width");
                    ui.add(
                        egui::DragValue::new(&mut self.paper_width_mm)
                            .speed(0.1)
                            .suffix(" mm"),
                    );
                    ui.end_row();

                    ui.label("Left margin");
                    ui.add(
                        egui::DragValue::new(&mut self.side_margin_mm)
                            .speed(0.1)
                            .suffix(" mm"),
                    );
                    ui.end_row();
                });

            let content_width_mm = self.paper_width_mm - self.side_margin_mm;

            ui.small(format!(
                "Content width: {:.2} mm",
                content_width_mm.max(0.0),
            ));

            if let Some(env_path) = &self.env_path {
                ui.small(format!("Saved in: {}", env_path.display(),));
            }

            ui.add_space(6.0);

            ui.horizontal(|ui| {
                if ui.button("Save Settings").clicked() {
                    let settings = PrintSettings {
                        paper_width_mm: self.paper_width_mm,
                        side_margin_mm: self.side_margin_mm,
                    };

                    match settings.validate() {
                        Err(err) => {
                            self.settings_message = format!("Invalid settings: {err}",);
                        }

                        Ok(()) => {
                            if let Some(env_path) = self.env_path.as_ref() {
                                match save_print_settings_to_env(env_path, settings) {
                                    Ok(()) => {
                                        if let Ok(mut current) = self.print_settings.write() {
                                            *current = settings;
                                        }

                                        self.settings_message = format!(
                                            "Saved: {:.2} mm paper, {:.2} mm side margin",
                                            settings.paper_width_mm, settings.side_margin_mm,
                                        );
                                    }

                                    Err(err) => {
                                        self.settings_message =
                                            format!("Unable to save settings: {err:#}",);
                                    }
                                }
                            } else {
                                self.settings_message =
                                    "Configuration is not loaded yet.".to_string();
                            }
                        }
                    }
                }

                if ui.button("Reset 76 / 2.5").clicked() {
                    self.paper_width_mm = DEFAULT_PAPER_WIDTH_MM;

                    self.side_margin_mm = DEFAULT_SIDE_MARGIN_MM;
                }
            });

            if !self.settings_message.is_empty() {
                ui.small(&self.settings_message);
            }

            ui.add_space(12.0);

            ui.separator();

            ui.add_space(8.0);

            ui.small("Socket.IO v1 / Engine.IO v3 compatibility mode");
        });

        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}
