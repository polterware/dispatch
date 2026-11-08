mod backends;
mod bench;
mod cli;
mod config;
mod download;
mod engine;
mod http;
mod models;
mod rss;

use anyhow::Result;
use clap::Parser;
use std::{
    fs::File,
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex},
};
use voice_core::{discovery, paths};

#[derive(Clone)]
struct TeeWriter(Arc<Mutex<File>>);
impl Write for TeeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        io::stderr().write_all(bytes)?;
        let mut file = self
            .0
            .lock()
            .map_err(|_| io::Error::other("log file lock poisoned"))?;
        file.write_all(bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()?;
        self.0
            .lock()
            .map_err(|_| io::Error::other("log file lock poisoned"))?
            .flush()
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("voice-engine: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ort=warn,whisper_rs=warn")),
        );
    if std::io::stderr().is_terminal() {
        let _ = subscriber.try_init();
    } else {
        std::fs::create_dir_all(paths::log_file().parent().expect("log parent"))?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths::log_file())?;
        let file = Arc::new(Mutex::new(file));
        let _ = subscriber
            .with_writer(move || TeeWriter(file.clone()))
            .try_init();
    }
    match cli::Cli::parse().command.unwrap_or_default() {
        cli::Command::Serve {
            port,
            if_not_running,
        } => http::serve(port, if_not_running).await,
        cli::Command::Status => {
            let info = discovery::read(&paths::discovery_file())?;
            let response = ureq::get(&format!("{}/v1/status", info.base_url()))
                .set("Authorization", &format!("Bearer {}", info.token))
                .call()?
                .into_string()?;
            println!("{response}");
            Ok(())
        }
        cli::Command::Stop => {
            let info = discovery::read(&paths::discovery_file())?;
            ureq::post(&format!("{}/v1/shutdown", info.base_url()))
                .set("Authorization", &format!("Bearer {}", info.token))
                .call()?;
            Ok(())
        }
        cli::Command::Doctor => http::doctor(),
        cli::Command::Models {
            command: cli::ModelsCommand::List,
        } => models::list(),
        cli::Command::Models {
            command:
                cli::ModelsCommand::Download {
                    yes,
                    stt,
                    no_tts,
                    no_vad,
                },
        } => download::download(yes, stt.as_deref(), no_tts, no_vad),
        cli::Command::Bench { wav, stt_model } => bench::run(wav, stt_model),
    }
}
