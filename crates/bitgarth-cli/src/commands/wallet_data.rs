use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::context::{CliError, Context, decode};
use crate::ui;

const MAX_ARCHIVE_BYTES: usize = 10 * 1024 * 1024;

fn password(confirm: bool) -> Result<Zeroizing<String>, CliError> {
    let value = if ui::stdin_is_interactive() {
        let first =
            Zeroizing::new(ui::prompt_secret("Archive password: ")?.ok_or(CliError::Cancelled)?);
        if confirm {
            let second = Zeroizing::new(
                ui::prompt_secret("Confirm archive password: ")?.ok_or(CliError::Cancelled)?,
            );
            if first != second {
                return Err(CliError::Usage("The archive passwords differ.".to_owned()));
            }
        }
        first
    } else {
        let mut line = Zeroizing::new(String::new());
        std::io::stdin().take(65537).read_to_string(&mut line)?;
        if line.len() > 65536 {
            return Err(CliError::Usage(
                "Archive password input is too long.".to_owned(),
            ));
        }
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        line
    };
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err(CliError::Usage(
            "Supply one non-empty archive password on stdin or at the prompt.".to_owned(),
        ));
    }
    Ok(value)
}

#[derive(Deserialize)]
struct Download {
    zip_base64: String,
    summary: Value,
}

fn is_encrypted_wallet_archive(bytes: &[u8]) -> bool {
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return false;
    };
    zip.len() == 1
        && zip.by_index_raw(0).is_ok_and(|file| {
            file.encrypted()
                && file.is_file()
                && file.name().is_ok_and(|name| name == "wallet-data.json")
        })
}

pub(super) fn export(context: &Context, file: &Path) -> Result<(), CliError> {
    let session = context.session()?;
    let dir = file
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if file.try_exists().map_err(file_error)? {
        return Err(CliError::Usage(
            "The destination already exists. Choose a new file.".to_owned(),
        ));
    }
    let mut staging = tempfile::NamedTempFile::new_in(dir).map_err(file_error)?;
    let password = password(true)?;
    let raw = session.send(
        Method::POST,
        &["exports", "wallet-data"],
        &json!({"password":password.as_str()}),
    )?;
    let download: Download = decode(raw)?;
    let bytes = STANDARD.decode(download.zip_base64).map_err(|_| {
        CliError::Failed("The server returned an invalid wallet archive.".to_owned())
    })?;
    if download.summary["encrypted"] != true
        || !is_encrypted_wallet_archive(&bytes)
        || bytes.len() > MAX_ARCHIVE_BYTES
    {
        return Err(CliError::Failed(
            "The server returned an invalid encrypted wallet archive.".to_owned(),
        ));
    }
    staging.write_all(&bytes).map_err(file_error)?;
    staging.as_file().sync_all().map_err(file_error)?;
    staging
        .persist_noclobber(file)
        .map_err(|error| file_error(error.error))?;
    if context.json() {
        return context.print_json(&json!({"file":file, "summary":download.summary}));
    }
    ui::success("Saved the encrypted wallet-data ZIP.");
    Ok(())
}

fn file_error(error: std::io::Error) -> CliError {
    CliError::Failed(format!("Wallet archive file error: {error}"))
}

pub(super) fn import(context: &Context, file: &Path) -> Result<(), CliError> {
    let session = context.session()?;
    let mut bytes = Vec::new();
    File::open(file)
        .map_err(file_error)?
        .take((MAX_ARCHIVE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(file_error)?;
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(CliError::Usage(
            "The wallet archive exceeds 10 MiB.".to_owned(),
        ));
    }
    if !is_encrypted_wallet_archive(&bytes) {
        return Err(CliError::Usage(
            "Choose an encrypted wallet-data ZIP.".to_owned(),
        ));
    }
    let password = password(false)?;
    let result = session.send(
        Method::POST,
        &["imports", "wallet-data"],
        &json!({"password":password.as_str(), "payload_base64":STANDARD.encode(bytes)}),
    )?;
    if context.json() {
        return context.print_json(&result);
    }
    ui::success("Imported wallet records.");
    for warning in result["validation_warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        ui::warn(warning);
    }
    if result["sync_triggered"] == true {
        ui::hint("Blockchain sync was queued for the imported accounts.");
    }
    Ok(())
}
