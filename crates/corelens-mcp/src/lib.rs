//! Corelens as a Model Context Protocol server.
//!
//! `corelens-desktop-ai --mcp` is started by an MCP client (Claude Code,
//! Claude Desktop) and speaks MCP on its standard streams. It holds no
//! logins, opens no database and reaches no network: every tool call goes
//! over [`bridge`] to the Corelens window already running for this user,
//! which owns the vault, the project and the connections to devices, and
//! answers with redacted text. With the window closed, or its AI switch
//! off, each call says so.
//!
//! [`server`] is the protocol, [`tools`] the list of what can be asked.

pub mod bridge;
pub mod server;
pub mod tools;

pub use server::{Backend, BoxFuture, Outcome};

/// Serves MCP on stdin/stdout until stdin closes, every call forwarded to
/// the app whose data folder is `data_dir`. Nothing but MCP messages is
/// ever written to stdout.
pub fn run_stdio(data_dir: &std::path::Path) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let endpoint = bridge::endpoint(data_dir);
    runtime.block_on(async move {
        let server = server::Server::new(bridge::Client { endpoint });
        server.serve(tokio::io::BufReader::new(tokio::io::stdin()), tokio::io::stdout()).await;
    });
    Ok(())
}
