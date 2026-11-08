use anyhow::{Context, Result};
use std::{
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};

pub fn download_model<F>(
    spec: &'static crate::catalog::ModelSpec,
    root: &Path,
    mut progress: F,
) -> Result<()>
where
    F: FnMut(u64, u64),
{
    let mut prior = 0u64;
    for file in spec.files {
        let bytes = download_url(file.url, root.join(file.path), file.size, |n, _| {
            progress(prior + n, spec.size)
        })?;
        prior += bytes;
    }
    Ok(())
}

fn download_url<F>(url: &str, target: std::path::PathBuf, size: u64, mut progress: F) -> Result<u64>
where
    F: FnMut(u64, u64),
{
    if target.is_file() {
        return Ok(std::fs::metadata(target)?.len());
    }
    std::fs::create_dir_all(target.parent().context("model path has no parent")?)?;
    let part = target.with_file_name(format!(
        "{}.part",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    let result = (|| -> Result<u64> {
        let mut input = ureq::get(url).call()?.into_reader();
        let mut output = std::fs::File::create(&part)?;
        let mut buf = [0u8; 64 * 1024];
        let mut bytes = 0u64;
        let mut last = Instant::now();
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n])?;
            bytes += n as u64;
            if last.elapsed() >= Duration::from_millis(100) {
                progress(bytes, size);
                last = Instant::now();
            }
        }
        output.sync_all()?;
        std::fs::rename(&part, &target)?;
        progress(bytes, size);
        Ok(bytes)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(part);
        return Err(error).with_context(|| format!("downloading {url}"));
    }
    result
}

pub fn download(
    id: Option<&str>,
    yes: bool,
    stt: Option<&str>,
    no_tts: bool,
    no_vad: bool,
) -> Result<()> {
    let mut selected = Vec::new();
    if let Some(id) = id {
        selected.push(
            crate::catalog::find(id).ok_or_else(|| anyhow::anyhow!("unknown model id: {id}"))?,
        );
    } else {
        let stt_id = match stt.unwrap_or("small") {
            "base" => "whisper-base",
            "turbo" => "whisper-large-v3-turbo-q5_0",
            _ => "whisper-small-q5_1",
        };
        selected.push(crate::catalog::find(stt_id).unwrap());
        if !no_tts {
            selected.push(crate::catalog::find("kokoro-v1").unwrap());
        }
        if !no_vad {
            selected.push(crate::catalog::find("silero-vad-v5").unwrap());
        }
    }
    for model in &selected {
        println!("{} ({} bytes, {})", model.id, model.size, model.license);
    }
    if !yes {
        eprint!("Download these model files? [y/N] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            return Ok(());
        }
    }
    for model in selected {
        download_model(model, &voice_core::paths::models_dir(), |_, _| {})?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_http_download_renames_part_when_bind_is_available() {
        let listener = match std::net::TcpListener::bind(("127.0.0.1", 0)) {
            Ok(v) => v,
            Err(error) => {
                eprintln!("skipping local download test: sandbox refused local bind: {error}");
                return;
            }
        };
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::{Read, Write};
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                let body = b"fake-model";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
        });
        let root = std::env::temp_dir().join(format!("voice-download-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let target = root.join("fake/model.bin");
        download_url(
            &format!("http://{address}/model"),
            target.clone(),
            10,
            |_, _| {},
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"fake-model");
        assert!(!target.with_file_name("model.bin.part").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
