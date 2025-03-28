use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;
use tokio;

mod proton;
use crate::proton::{ProtonClient, ProtonServer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        println!("Usage: {} <server|client> [server_addr]", args[0]);
        return Ok(());
    }

    match args[1].as_str() {
        "server" => {
            println!("Starting Proton server...");
            let bind_addr: SocketAddr = "127.0.0.1:5000".parse()?;

            // Generate self-signed certificate for testing
            let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
            let key = rustls::PrivateKey(cert.serialize_private_key_der());
            let cert = rustls::Certificate(cert.serialize_der()?);

            let server = ProtonServer::new(bind_addr, cert, key)?;
            server.run().await?;
            Ok(())
        }
        "client" => {
            let server_addr: SocketAddr = if args.len() > 2 {
                args[2].parse()?
            } else {
                "127.0.0.1:5000".parse()?
            };

            let bind_addr: SocketAddr = "127.0.0.1:0".parse()?;
            println!("Connecting to Proton server at {}...", server_addr);

            let mut client = ProtonClient::new(bind_addr)?;
            let mut connection = client.connect(server_addr).await?;

            // Example: Send events and read actions in a loop
            for i in 0..5 {
                connection.send_event().await?;
                connection.send_state_commit(i).await?;
                connection.read_action().await?;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Ok(())
        }
        _ => {
            println!("Invalid command. Use 'server' or 'client'");
            Ok(())
        }
    }
}
