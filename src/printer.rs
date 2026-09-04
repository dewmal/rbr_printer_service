use crate::*;

// =========================================================
// WINDOWS PRINT
// =========================================================

#[cfg(target_os = "windows")]
pub(crate) fn print_pdf(path: &Path, paper_height_mm: f64, paper_width_mm: f64) -> Result<()> {
    let exe_path = env::current_exe().context("Unable to determine application path")?;

    let exe_dir = exe_path
        .parent()
        .context("Unable to determine application directory")?;

    let sumatra = exe_dir.join("SumatraPDF.exe");

    if !sumatra.exists() {
        bail!("SumatraPDF.exe not found at {}", sumatra.display());
    }

    println!("[PRINT] PDF: {}", path.display());
    println!("[PRINT] Sending to Windows default printer");

    // The prepared PDF already contains the configured left margin and
    // final 76 mm page width. Pass only that exact geometry to Sumatra;
    // do not ask Sumatra to center or rescale it again.
    let print_settings = format!(
        "noscale,paper={:.2}mm x {:.2}mm,disable-auto-rotation,ignore-pdf-print-settings",
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
