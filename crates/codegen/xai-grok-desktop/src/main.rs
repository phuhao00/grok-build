//! Grok Build native desktop client (egui + ACP stdio).

mod agent_bridge;
mod app;
mod config_io;
mod events;
mod fonts;
mod markdown;
mod model;

use std::path::PathBuf;

use clap::Parser;
use eframe::egui;

use crate::agent_bridge::{resolve_grok_bin, BridgeConfig};
use crate::app::GrokDesktopApp;

#[derive(Debug, Parser)]
#[command(
    name = "xai-grok-desktop",
    about = "Native desktop client for Grok Build"
)]
struct Args {
    /// Working directory for the agent session (default: current directory).
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// Path to the `grok` binary (default: search PATH).
    #[arg(long)]
    grok_bin: Option<PathBuf>,

    /// Require manual approval for tool permissions (default: auto-approve).
    #[arg(long = "ask-permissions")]
    ask_permissions: bool,
}

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let args = Args::parse();
    let cwd = args
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let grok_bin = resolve_grok_bin(args.grok_bin.as_deref());

    let config = BridgeConfig {
        grok_bin,
        cwd,
        // Default auto-approve so chat/tools feel interactive out of the box.
        always_approve: !args.ask_permissions,
    };

    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_title("Grok Desktop"),
        ..Default::default()
    };

    eframe::run_native(
        "Grok Desktop",
        native,
        Box::new(move |cc| Ok(Box::new(GrokDesktopApp::new(cc, config)))),
    )
}
