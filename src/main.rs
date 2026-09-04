#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use anyhow::{Context, Result, bail};
use eframe::egui;
use lopdf::{Dictionary, Document, Object, Stream};
use reqwest::blocking::Client;
use serde_json::Value;

use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, RwLock,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use tempfile::{Builder, NamedTempFile};
use tungstenite::{Message, connect};
use url::Url;

// =========================================================
// CONSTANTS
// =========================================================

const MAX_PDF_SIZE: u64 = 100 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

// Default thermal paper geometry. These can be edited in the UI and
// are persisted in the .env file.
const DEFAULT_PAPER_WIDTH_MM: f64 = 76.0;
const DEFAULT_SIDE_MARGIN_MM: f64 = 4.0;
const DEFAULT_PREVIEW_BEFORE_PRINT: bool = false;

const ENV_PAPER_WIDTH_MM: &str = "PAPER_WIDTH_MM";
const ENV_SIDE_MARGIN_MM: &str = "SIDE_MARGIN_MM";
const ENV_PREVIEW_BEFORE_PRINT: &str = "PREVIEW_BEFORE_PRINT";

mod config;
mod logging;
mod pdf;
mod printer;
mod ui;
mod worker;

use config::*;
use logging::*;
use ui::*;
use worker::*;

// =========================================================
// MAIN
// =========================================================

fn main() -> eframe::Result<()> {
    install_crash_logger();

    log_info("[APP] RBPrinter starting");

    let (ui_tx, ui_rx) = mpsc::channel::<WorkerEvent>();

    let print_settings = Arc::new(RwLock::new(PrintSettings::default()));

    let worker_ui_tx = ui_tx.clone();

    let worker_print_settings = print_settings.clone();

    thread::spawn(move || {
        log_info("[THREAD] service worker starting");

        let worker_result = panic::catch_unwind(AssertUnwindSafe(|| {
            service_worker(worker_ui_tx.clone(), worker_print_settings.clone());
        }));

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
            .with_inner_size([520.0, 650.0])
            .with_min_inner_size([520.0, 650.0])
            .with_resizable(false),

        ..Default::default()
    };

    log_info("[APP] starting GUI");

    let result = eframe::run_native(
        "PDF Printer Client",
        options,
        Box::new(move |_cc| Ok(Box::new(PrinterApp::new(ui_rx, print_settings.clone())))),
    );

    if let Err(err) = &result {
        log_error(&format!("[GUI ERROR] {err:?}"));
    }

    result
}
