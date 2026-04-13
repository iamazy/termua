use std::net::SocketAddr;

use anyhow::Context as _;
use clap::Parser;
use termua_relay::server::ServerConfig;

#[derive(Debug, Parser)]
#[command(name = "termua-relay")]
struct Args {
    /// Listen address, e.g. 127.0.0.1:7231
    #[arg(long, default_value = "127.0.0.1:7231")]
    listen: SocketAddr,

    /// Drop input events from non-controllers.
    #[arg(long, default_value_t = true)]
    gate_input: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    termua_relay::server::serve(
        args.listen,
        ServerConfig {
            gate_input: args.gate_input,
        },
    )
    .await
    .with_context(|| format!("failed to serve on {}", args.listen))?;

    Ok(())
}
