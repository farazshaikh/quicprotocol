use clap::Parser;
use rustls::RootCertStore;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_CONNS: u32 = 1;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Run as server (if not set, runs as client)
    #[arg(short, long)]
    server: bool,

    /// Server address to connect to or listen on
    #[arg(short, long, default_value = "127.0.0.1:4433")]
    addr: String,

    /// Message to send (client only)
    #[arg(short, long, default_value = "Hello from QUIC client!")]
    message: String,
}

fn generate_cert() -> (rustls::Certificate, rustls::PrivateKey) {
    let x = rcgen::generate_simple_self_signed(["localhost".into()]).unwrap();
    let key = x.serialize_private_key_der();
    let cert = x.serialize_der().unwrap();
    (rustls::Certificate(cert), rustls::PrivateKey(key))
}

async fn run_server(addr: std::net::SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let (cert, key) = generate_cert();

    // Configure TLS settings
    let mut rustls_conf = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    rustls_conf.alpn_protocols = vec![b"proton".to_vec()];

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(rustls_conf));
    let mut transport_config = quinn::TransportConfig::default();

    // Set keep-alive and timeout settings
    transport_config
        .keep_alive_interval(Some(std::time::Duration::from_secs(1)))
        .max_idle_timeout(Some(quinn::VarInt::from_u32(90_000).into())); // 90s

    server_config.concurrent_connections(MAX_CONNS);
    server_config.transport_config(Arc::new(transport_config));

    let endpoint = quinn::Endpoint::server(server_config, addr)?;
    println!("Listening on {}", endpoint.local_addr()?);

    while let Some(conn) = endpoint.accept().await {
        tokio::spawn(async move {
            match handle_connection(conn).await {
                Ok(_) => println!("Connection handled successfully"),
                Err(e) => eprintln!("Connection error: {}", e),
            }
        });
    }

    Ok(())
}

async fn handle_connection(conn: quinn::Connecting) -> Result<(), Box<dyn std::error::Error>> {
    let connection = conn.await?;
    println!(
        "Connection established from {}",
        connection.remote_address()
    );

    while let Ok((mut send, mut recv)) = connection.accept_bi().await {
        println!("New bidirectional stream");
        loop {
            // Read a u32 value
            let value = match recv.read_u32_le().await {
                Ok(val) => {
                    println!("Received u32: {}", val);
                    val
                }
                Err(e) => {
                    println!("Error reading u32: {}", e);
                    break;
                }
            };

            // Echo the same u32 back
            if let Err(e) = send.write_u32(value).await {
                println!("Error sending response: {}", e);
                break;
            }
            println!("Echoed back u32: {}", value);
        }
    }

    Ok(())
}
struct SkipServerVerification;
impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

async fn run_client(
    addr: std::net::SocketAddr,
    message: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut client_config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();

    // Set ALPN protocol
    client_config.alpn_protocols = vec![b"proton".to_vec()];

    let mut client_config = quinn::ClientConfig::new(Arc::new(client_config));
    let mut transport_config = quinn::TransportConfig::default();

    // Set keep-alive and timeout settings
    transport_config
        .keep_alive_interval(Some(std::time::Duration::from_secs(1)))
        .max_idle_timeout(Some(quinn::VarInt::from_u32(90_000).into())); // 90s

    client_config.transport_config(Arc::new(transport_config));

    // Create endpoint
    let mut endpoint = quinn::Endpoint::client(([0, 0, 0, 0], 0).into())?;
    endpoint.set_default_client_config(client_config);

    // Connect to server
    let connection = endpoint.connect(addr, "localhost")?.await?;
    println!("Connected to server");

    // Open bidirectional stream
    let (mut send, mut recv) = connection.open_bi().await?;

    // Send message in a loop
    let mut counter = 0;
    loop {
        println!("Loop {}", counter);

        // Prepare a u32 message (counter)
        let value = counter as u32;
        let bytes = value.to_le_bytes();
        // Send the u32 value
        send.write_all(&bytes).await?;
        println!("Sent u32 value: {}", value);

        counter += 1;
        tokio::time::sleep(tokio::time::Duration::from_micros(100)).await;

        // Read u32 response
        let mut buf = [0u8; 4]; // Exactly 4 bytes for u32
        match recv.read_exact(&mut buf).await {
            Ok(()) => {
                let response = u32::from_le_bytes(buf);
                println!("Received u32 response: {}", response);
            }
            Err(e) => {
                println!("Error reading response: {}", e);
                break;
            }
        }
    }
    println!("Finished sending messages");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let addr: std::net::SocketAddr = args.addr.parse()?;

    if args.server {
        println!("Starting server...");
        run_server(addr).await?;
    } else {
        println!("Starting client...");
        run_client(addr, args.message).await?;
    }

    Ok(())
}
