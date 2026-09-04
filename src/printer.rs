use crate::*;

// =========================================================
// WINDOWS PRINT
// =========================================================

#[cfg(target_os = "windows")]
fn sumatra_path() -> Result<PathBuf> {
    let exe_path = env::current_exe().context("Unable to determine application path")?;
    let exe_dir = exe_path
        .parent()
        .context("Unable to determine application directory")?;
    let sumatra = exe_dir.join("SumatraPDF.exe");

    if !sumatra.exists() {
        bail!("SumatraPDF.exe not found at {}", sumatra.display());
    }

    Ok(sumatra)
}

#[cfg(target_os = "windows")]
pub(crate) fn preview_pdf(path: &Path) -> Result<()> {
    let sumatra = sumatra_path()?;

    println!("[PREVIEW] Opening {}", path.display());

    let status = Command::new(sumatra)
        .arg("-new-window")
        .arg("-esc-to-exit")
        .arg(path)
        .status()
        .context("Unable to open SumatraPDF preview")?;

    if !status.success() {
        bail!("Preview failed. SumatraPDF exit code: {:?}", status.code());
    }

    println!("[PREVIEW] Closed; continuing to print");
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn print_pdf(path: &Path, paper_height_mm: f64, paper_width_mm: f64) -> Result<()> {
    let sumatra = sumatra_path()?;

    println!("[PRINT] PDF: {}", path.display());
    println!("[PRINT] Sending to Windows default printer");

    // Let Sumatra select a form matching the prepared PDF's exact dynamic
    // dimensions. Keep the prepared PDF at exact size so the driver does not
    // vertically center it inside a longer default form and feed a large blank
    // area first. Centering here is horizontal only; it moves the page away
    // from the printer's clipped left origin without changing its top position.
    let print_settings =
        "noscale,center,paper=auto,bin=auto,disable-auto-rotation,ignore-pdf-print-settings";

    println!(
        "[PRINT] Requested paper: {:.2} mm x {:.2} mm",
        paper_width_mm, paper_height_mm,
    );
    println!("[PRINT] Settings: {}", print_settings,);

    let status = Command::new(&sumatra)
        .arg("-print-to-default")
        .arg("-print-settings")
        .arg(&print_settings)
        .arg("-silent")
        .arg(path)
        .status()
        .context("Unable to start SumatraPDF")?;

    if !status.success() {
        bail!("Printing failed. SumatraPDF exit code: {:?}", status.code());
    }

    println!("[PRINT] PDF submitted to default printer");

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn preview_pdf(_path: &Path) -> Result<()> {
    bail!("Print preview is currently supported only on Windows")
}

// =========================================================
// LINUX PRINT
// =========================================================

#[cfg(target_os = "linux")]
pub(crate) fn print_pdf(path: &Path, _paper_height_mm: f64, _paper_width_mm: f64) -> Result<()> {
    let status = Command::new("lp")
        .arg(path)
        .status()
        .context("Unable to execute lp")?;

    if !status.success() {
        bail!("lp print command failed");
    }

    Ok(())
}

// =========================================================
// MACOS PRINT
// =========================================================

#[cfg(target_os = "macos")]
pub(crate) fn print_pdf(path: &Path, _paper_height_mm: f64, _paper_width_mm: f64) -> Result<()> {
    let status = Command::new("lp")
        .arg(path)
        .status()
        .context("Unable to execute lp")?;

    if !status.success() {
        bail!("macOS print command failed");
    }

    Ok(())
}

// =========================================================
// UNSUPPORTED PLATFORM
// =========================================================

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub(crate) fn print_pdf(_path: &Path, _paper_height_mm: f64, _paper_width_mm: f64) -> Result<()> {
    bail!("Unsupported operating system");
}
