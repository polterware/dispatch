use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "dispatch-voice", version, about = "Local voice engine")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Subcommand)]
pub enum Command {
    Serve {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        if_not_running: bool,
    },
    Status,
    Stop,
    Doctor,
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    Bench {
        #[arg(long)]
        wav: Option<PathBuf>,
        #[arg(long)]
        stt_model: Option<PathBuf>,
    },
}
impl Default for Command {
    fn default() -> Self {
        Self::Serve {
            port: None,
            if_not_running: false,
        }
    }
}
#[derive(Subcommand)]
pub enum ModelsCommand {
    List,
    Download {
        id: Option<String>,
        #[arg(long)]
        yes: bool,
        #[arg(long, value_parser=["small", "base", "turbo"])]
        stt: Option<String>,
        #[arg(long)]
        no_tts: bool,
        #[arg(long)]
        no_vad: bool,
    },
    Remove {
        id: String,
    },
}
