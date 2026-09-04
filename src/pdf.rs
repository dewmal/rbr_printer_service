use crate::*;

// =========================================================
// PREPARE RECEIPT PDF
// =========================================================

pub(crate) fn pdf_number_to_f64(object: &Object) -> Result<f64> {
    match object {
        Object::Integer(value) => Ok(*value as f64),

        Object::Real(value) => Ok(*value as f64),

        _ => bail!("PDF page box contains a non-numeric value"),
    }
}

pub(crate) fn get_inherited_page_value<'a>(
    document: &'a Document,
    start_id: lopdf::ObjectId,
    key: &[u8],
) -> Option<&'a Object> {
    let mut current_id = Some(start_id);

    while let Some(object_id) = current_id {
        let dictionary = document.get_object(object_id).ok()?.as_dict().ok()?;

        if let Ok(value) = dictionary.get(key) {
            return Some(value);
        }

        current_id = dictionary
            .get(b"Parent")
            .ok()
            .and_then(|object| object.as_reference().ok());
    }

    None
}

pub(crate) fn read_page_box(
    document: &Document,
    page_id: lopdf::ObjectId,
) -> Result<(f64, f64, f64, f64)> {
    // CropBox and MediaBox may be inherited from the page tree.
    // Prefer CropBox because it represents the visible page area.
    let page_box_object = get_inherited_page_value(document, page_id, b"CropBox")
        .or_else(|| get_inherited_page_value(document, page_id, b"MediaBox"))
        .context("PDF page has no CropBox or MediaBox")?;

    let page_box = page_box_object
        .as_array()
        .context("PDF page box is not an array")?;

    if page_box.len() != 4 {
        bail!("PDF page box must contain exactly 4 values");
    }

    let x0 = pdf_number_to_f64(&page_box[0])?;
    let y0 = pdf_number_to_f64(&page_box[1])?;
    let x1 = pdf_number_to_f64(&page_box[2])?;
    let y1 = pdf_number_to_f64(&page_box[3])?;

    Ok((x0, y0, x1, y1))
}

pub(crate) fn append_original_contents(
    document: &mut Document,
    original_contents: Option<Object>,
    new_contents: &mut Vec<Object>,
) -> Result<()> {
    let Some(contents) = original_contents else {
        return Ok(());
    };

    match contents {
        Object::Reference(object_id) => {
            new_contents.push(Object::Reference(object_id));
        }

        Object::Array(items) => {
            for item in items {
                match item {
                    Object::Reference(object_id) => {
                        new_contents.push(Object::Reference(object_id));
                    }

                    Object::Stream(stream) => {
                        let object_id = document.add_object(stream);

                        new_contents.push(Object::Reference(object_id));
                    }

                    _ => {
                        bail!("Unsupported PDF Contents array item");
                    }
                }
            }
        }

        Object::Stream(stream) => {
            let object_id = document.add_object(stream);

            new_contents.push(Object::Reference(object_id));
        }

        Object::Null => {}

        _ => {
            bail!("Unsupported PDF Contents object");
        }
    }

    Ok(())
}

pub(crate) struct PreparedReceiptPdf {
    pub(crate) file: NamedTempFile,
    pub(crate) paper_height_mm: f64,
}

pub(crate) fn prepare_receipt_pdf(
    source_path: &Path,
    settings: PrintSettings,
) -> Result<PreparedReceiptPdf> {
    settings.validate()?;
    let mut document =
        Document::load(source_path).context("Unable to open PDF for receipt margin preparation")?;

    let pages = document.get_pages();

    if pages.is_empty() {
        bail!("PDF contains no pages");
    }

    // Thermal receipt jobs must be one continuous page because the printer
    // needs one exact custom paper height for the job.
    if pages.len() != 1 {
        bail!(
            "Receipt PDF must contain exactly one page; found {} pages",
            pages.len(),
        );
    }

    let paper_width_pt = settings.paper_width_mm * 72.0 / 25.4;

    let left_margin_pt = settings.side_margin_mm * 72.0 / 25.4;

    let content_width_pt = settings.content_width_mm() * 72.0 / 25.4;

    let mut prepared_height_mm = 0.0_f64;

    for (_page_number, page_id) in pages {
        let (x0, y0, x1, y1) = read_page_box(&document, page_id)?;

        let source_width_pt = (x1 - x0).abs();

        let source_height_pt = (y1 - y0).abs();

        if source_width_pt <= 0.0 || source_height_pt <= 0.0 {
            bail!("PDF page has invalid dimensions");
        }

        // Uniformly scale only when necessary. The content is never enlarged.
        // The configured margin is reserved only on the LEFT. The receipt uses
        // all remaining width up to the right edge. Height follows the same
        // uniform scale and therefore remains dynamic for every PDF.
        let scale = (content_width_pt / source_width_pt).min(1.0);

        let output_height_pt = source_height_pt * scale;

        prepared_height_mm = output_height_pt * 25.4 / 72.0;

        // Place the scaled source at the configured LEFT margin.
        // Do not center it; the remaining space is intentionally used on the right.
        let translate_x = left_margin_pt - (x0 * scale);

        let translate_y = -(y0 * scale);

        let prefix = format!(
            "q\n{:.8} 0 0 {:.8} {:.8} {:.8} cm\n",
            scale, scale, translate_x, translate_y,
        );

        let prefix_id = document.add_object(Stream::new(Dictionary::new(), prefix.into_bytes()));

        let suffix_id = document.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));

        let original_contents = {
            let page = document
                .get_object(page_id)
                .context("Unable to read PDF page contents")?
                .as_dict()
                .context("PDF page object is not a dictionary")?;

            page.get(b"Contents").ok().cloned()
        };

        let mut new_contents = vec![Object::Reference(prefix_id)];

        append_original_contents(&mut document, original_contents, &mut new_contents)?;

        new_contents.push(Object::Reference(suffix_id));

        let page = document
            .get_object_mut(page_id)
            .context("Unable to update PDF page")?
            .as_dict_mut()
            .context("PDF page object is not a dictionary")?;

        let new_page_box = Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Real(paper_width_pt as f32),
            Object::Real(output_height_pt as f32),
        ]);

        page.set("MediaBox", new_page_box.clone());

        page.set("CropBox", new_page_box);

        page.set("Contents", Object::Array(new_contents));

        // The content stream now has the final orientation and position.
        // Remove page rotation so the printer does not rotate it again.
        page.set("Rotate", Object::Integer(0));

        println!(
            "[PDF PREP] Source: {:.2} mm x {:.2} mm",
            source_width_pt * 25.4 / 72.0,
            source_height_pt * 25.4 / 72.0,
        );

        println!(
            "[PDF PREP] Output: {:.2} mm x {:.2} mm, scale {:.4}",
            settings.paper_width_mm,
            output_height_pt * 25.4 / 72.0,
            scale,
        );
    }

    // Remove unused objects after replacing page content streams.
    document.prune_objects();

    let mut prepared = Builder::new()
        .prefix("rbprinter_prepared_")
        .suffix(".pdf")
        .tempfile()
        .context("Unable to create prepared PDF temporary file")?;

    document
        .save_to(prepared.as_file_mut())
        .context("Unable to save prepared receipt PDF")?;

    prepared
        .as_file_mut()
        .flush()
        .context("Unable to flush prepared receipt PDF")?;

    prepared
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .context("Unable to rewind prepared receipt PDF")?;

    Ok(PreparedReceiptPdf {
        file: prepared,
        paper_height_mm: prepared_height_mm,
    })
}

// =========================================================
// DOWNLOAD PDF
// =========================================================

pub(crate) fn download_pdf(http_client: &Client, url_string: &str) -> Result<NamedTempFile> {
    let url =
        Url::parse(url_string).with_context(|| format!("Invalid PDF URL: {}", url_string,))?;

    match url.scheme() {
        "http" | "https" => {}

        scheme => {
            bail!("Unsupported URL protocol: {}", scheme,);
        }
    }

    // ---------------------------------------------
    // REQUEST
    // ---------------------------------------------

    let response = http_client
        .get(url)
        .send()
        .context("Unable to download PDF")?
        .error_for_status()
        .context("PDF server returned an HTTP error")?;

    // ---------------------------------------------
    // SIZE
    // ---------------------------------------------

    if let Some(size) = response.content_length() {
        println!("[DOWNLOAD] Content length: {} bytes", size,);

        if size > MAX_PDF_SIZE {
            bail!(
                "PDF exceeds maximum size of {} MB",
                MAX_PDF_SIZE / 1024 / 1024,
            );
        }
    }

    // ---------------------------------------------
    // TEMP FILE
    // ---------------------------------------------

    let mut temp_file = Builder::new()
        .prefix("rbprinter_")
        .suffix(".pdf")
        .tempfile()
        .context("Unable to create temporary PDF file")?;

    // ---------------------------------------------
    // DOWNLOAD
    // ---------------------------------------------

    let mut limited = response.take(MAX_PDF_SIZE + 1);

    let downloaded = io::copy(&mut limited, temp_file.as_file_mut())
        .context("Error writing PDF to temporary file")?;

    println!("[DOWNLOAD] {} bytes downloaded", downloaded,);

    if downloaded > MAX_PDF_SIZE {
        bail!("Downloaded PDF exceeds {} MB", MAX_PDF_SIZE / 1024 / 1024,);
    }

    temp_file
        .as_file_mut()
        .flush()
        .context("Unable to flush downloaded PDF")?;

    // ---------------------------------------------
    // VALIDATE %PDF-
    // ---------------------------------------------

    temp_file.as_file_mut().seek(SeekFrom::Start(0))?;

    let mut header = [0_u8; 5];

    temp_file
        .as_file_mut()
        .read_exact(&mut header)
        .context("Downloaded file is too small to be a PDF")?;

    if &header != b"%PDF-" {
        bail!("Downloaded file is not a valid PDF");
    }

    temp_file.as_file_mut().seek(SeekFrom::Start(0))?;

    println!("[DOWNLOAD] PDF validation successful");

    Ok(temp_file)
}
