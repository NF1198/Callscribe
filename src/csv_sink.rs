use crate::errors::AppError;
use crate::model::RadioRecord;
use log::info;
use std::path::Path;
use tokio::fs::File;
use tokio::io::BufWriter;
use tokio::sync::mpsc::Receiver;
use tokio_util::compat::TokioAsyncWriteCompatExt; // <- compat bridge

fn excel_guard_radio_type(s: &str) -> String {
    // Per your request, drop the leading '+' entirely
    s.trim_start_matches('+').to_string()
}

pub async fn write_csv_stream(
    out_path: &Path,
    mut rx: Receiver<RadioRecord>,
) -> Result<(), AppError> {
    let file = File::create(out_path)
        .await
        .map_err(|e| AppError::IO(format!("open out csv '{}': {}", out_path.display(), e)))?;
    let writer = BufWriter::new(file);

    // Bridge Tokio AsyncWrite -> futures::io::AsyncWrite for csv_async
    let compat_writer = writer.compat_write();
    let mut wtr = csv_async::AsyncWriter::from_writer(compat_writer);

    // header once
    wtr.write_record(&[
        "record_number",
        "datetime",
        "duration",
        "frequency",
        "radio_type",
        "dcc",
        "slot1_tg",
        "slot1_rid",
        "slot1_text",
        "slot2_tg",
        "slot2_rid",
        "slot2_text",
    ])
    .await
    .map_err(|e| AppError::IO(format!("csv write header: {}", e)))?;

    let mut count: usize = 0;

    while let Some(r) = rx.recv().await {
        let row = [
            r.record_number.to_string(),
            r.datetime.format("%Y-%m-%d %H:%M:%S").to_string(),
            r.duration.to_string(),
            r.frequency.clone().unwrap_or_default(),
            excel_guard_radio_type(r.radio_type.as_deref().unwrap_or("")),
            r.dcc.clone().unwrap_or_default(),
            r.slot1.tg.clone().unwrap_or_default(),
            r.slot1.rid.clone().unwrap_or_default(),
            r.slot1.text.clone().unwrap_or_default(),
            r.slot2.tg.clone().unwrap_or_default(),
            r.slot2.rid.clone().unwrap_or_default(),
            r.slot2.text.clone().unwrap_or_default(),
        ];

        wtr.write_record(&row)
            .await
            .map_err(|e| AppError::IO(format!("csv write row: {}", e)))?;
        count += 1;
    }

    wtr.flush()
        .await
        .map_err(|e| AppError::IO(format!("csv flush: {}", e)))?;

    info!("CSV wrote {} rows to {}", count, out_path.display());
    Ok(())
}
