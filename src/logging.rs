use crate::*;

// =========================================================
// LOGGING / CRASH DIAGNOSTICS
// =========================================================

pub(crate) fn log_file_path(file_name: &str) -> Option<std::path::PathBuf> {
    let exe_path = env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;

    Some(exe_dir.join(file_name))
}

pub(crate) fn append_to_file(file_name: &str, message: &str) {
    let Some(path) = log_file_path(file_name) else {
        return;
    };

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{:?} {}", std::time::SystemTime::now(), message,);
    }
}

pub(crate) fn log_info(message: &str) {
    println!("{}", message);
    append_to_file("rbprinter.log", message);
}

pub(crate) fn log_error(message: &str) {
    eprintln!("{}", message);
    append_to_file("rbprinter.log", message);
}

pub(crate) fn install_crash_logger() {
    panic::set_hook(Box::new(|panic_info| {
        let backtrace = std::backtrace::Backtrace::force_capture();

        let message = format!("[PANIC] {}\nBACKTRACE:\n{}", panic_info, backtrace,);

        eprintln!("{}", message);

        append_to_file("rbprinter-crash.log", &message);

        append_to_file("rbprinter.log", &message);
    }));
}

// =========================================================
// STATUS
// =========================================================

pub(crate) fn send_status(
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

    let log_message = format!("{} {}", prefix, message,);

    match &status {
        PrinterStatus::Error | PrinterStatus::Disconnected => {
            log_error(&log_message);
        }

        _ => {
            log_info(&log_message);
        }
    }

    let _ = tx.send(WorkerEvent::Status(status));

    let _ = tx.send(WorkerEvent::Message(message));
}
