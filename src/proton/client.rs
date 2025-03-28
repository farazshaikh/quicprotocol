use crate::proton::{
    ProtonError, IDLE_TIMEOUT, MAX_BIDIRECTIONAL_STREAMS, STARTUP_DELAY, STREAM_ACTION,
    STREAM_EVENT, STREAM_STATE_COMMIT, STREAM_TIMEOUT,
};
use quinn::{ClientConfig, Connection as QuinnConnection, Endpoint, RecvStream, SendStream};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::time::{sleep, timeout};

struct StreamPair {
    send: SendStream,
    recv: RecvStream,
}

struct ProtonStreamHandler {
    connection: QuinnConnection,
    event_stream: Option<StreamPair>,
    state_commit_stream: Option<StreamPair>,
    action_stream: Option<StreamPair>,
}

impl ProtonStreamHandler {
    fn new(connection: QuinnConnection) -> Self {
        Self {
            connection,
            event_stream: None,
            state_commit_stream: None,
            action_stream: None,
        }
    }

    async fn establish_streams(&mut self) -> Result<(), ProtonError> {
        // Open event stream
        let (mut send, recv) = self.connection.open_bi().await?;
        println!("Opening event stream...");
        timeout(STREAM_TIMEOUT, send.write_all(&[STREAM_EVENT])).await??;
        self.event_stream = Some(StreamPair { send, recv });
        println!("Event stream established");

        // Open state commit stream
        let (mut send, recv) = self.connection.open_bi().await?;
        println!("Opening state commit stream...");
        timeout(STREAM_TIMEOUT, send.write_all(&[STREAM_STATE_COMMIT])).await??;
        self.state_commit_stream = Some(StreamPair { send, recv });
        println!("State commit stream established");

        // Open action stream
        let (mut send, recv) = self.connection.open_bi().await?;
        println!("Opening action stream...");
        timeout(STREAM_TIMEOUT, send.write_all(&[STREAM_ACTION])).await??;
        self.action_stream = Some(StreamPair { send, recv });
        println!("Action stream established");

        Ok(())
    }

    async fn send_event(&mut self, event_id: u32) -> Result<u32, ProtonError> {
        if let Some(StreamPair {
            ref mut send,
            ref mut recv,
        }) = self.event_stream
        {
            timeout(STREAM_TIMEOUT, send.write_all(&event_id.to_le_bytes())).await??;
            let mut response = [0u8; 4];
            timeout(STREAM_TIMEOUT, recv.read_exact(&mut response)).await??;
            Ok(u32::from_le_bytes(response))
        } else {
            Err(ProtonError::InvalidStream)
        }
    }

    async fn send_state_commit(&mut self, commit_id: u32) -> Result<u32, ProtonError> {
        if let Some(StreamPair {
            ref mut send,
            ref mut recv,
        }) = self.state_commit_stream
        {
            timeout(STREAM_TIMEOUT, send.write_all(&commit_id.to_le_bytes())).await??;
            let mut response = [0u8; 4];
            timeout(STREAM_TIMEOUT, recv.read_exact(&mut response)).await??;
            Ok(u32::from_le_bytes(response))
        } else {
            Err(ProtonError::InvalidStream)
        }
    }

    async fn read_action(&mut self) -> Result<u32, ProtonError> {
        if let Some(StreamPair {
            ref mut send,
            ref mut recv,
        }) = self.action_stream
        {
            let request_id = 42u32; // Example request ID
            timeout(STREAM_TIMEOUT, send.write_all(&request_id.to_le_bytes())).await??;
            let mut data = [0u8; 4];
            timeout(STREAM_TIMEOUT, recv.read_exact(&mut data)).await??;
            Ok(u32::from_le_bytes(data))
        } else {
            Err(ProtonError::InvalidStream)
        }
    }
}

pub struct ProtonClient {
    endpoint: Endpoint,
    last_event_id: u32,
}

impl ProtonClient {
    pub fn new(bind_addr: SocketAddr) -> Result<Self, ProtonError> {
        // Configure TLS (skip verification since we're on localhost)
        let mut client_crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
            .with_no_client_auth();
        client_crypto.alpn_protocols = vec![b"proton".to_vec()];

        // Configure QUIC client
        let mut client_config = ClientConfig::new(Arc::new(client_crypto));
        let mut transport_config = quinn::TransportConfig::default();
        transport_config
            .keep_alive_interval(Some(std::time::Duration::from_secs(5)))
            .max_idle_timeout(Some(IDLE_TIMEOUT.try_into().unwrap()))
            .max_concurrent_bidi_streams(MAX_BIDIRECTIONAL_STREAMS.into());
        client_config.transport_config(Arc::new(transport_config));

        // Create endpoint
        let mut endpoint = Endpoint::client(bind_addr)?;
        endpoint.set_default_client_config(client_config);

        Ok(ProtonClient {
            endpoint,
            last_event_id: 0,
        })
    }

    pub async fn connect(
        &mut self,
        server_addr: SocketAddr,
    ) -> Result<ProtonConnection<'_>, ProtonError> {
        // Wait for startup delay to ensure old connections are cleaned up
        println!(
            "Waiting {} seconds for startup delay...",
            STARTUP_DELAY.as_secs()
        );
        sleep(STARTUP_DELAY).await;

        // Connect to server
        let connection = self.endpoint.connect(server_addr, "localhost")?.await?;
        println!("Connected to server at {}", server_addr);

        // Create protocol client
        let mut handler = ProtonStreamHandler::new(connection.clone());

        // Establish all streams
        handler.establish_streams().await?;
        println!("All streams established");

        Ok(ProtonConnection {
            handler,
            last_event_id: &mut self.last_event_id,
        })
    }
}

pub struct ProtonConnection<'a> {
    handler: ProtonStreamHandler,
    last_event_id: &'a mut u32,
}

impl<'a> ProtonConnection<'a> {
    pub async fn send_event(&mut self) -> Result<u32, ProtonError> {
        *self.last_event_id += 1;
        let event_id = *self.last_event_id;
        match self.handler.send_event(event_id).await {
            Ok(ack) => {
                println!("Event {} acknowledged with {}", event_id, ack);
                Ok(ack)
            }
            Err(e) => {
                eprintln!("Failed to send event {}: {}", event_id, e);
                Err(e)
            }
        }
    }

    pub async fn send_state_commit(&mut self, commit_id: u32) -> Result<u32, ProtonError> {
        match self.handler.send_state_commit(commit_id).await {
            Ok(response) => {
                println!(
                    "State commit {} completed with response {}",
                    commit_id, response
                );
                Ok(response)
            }
            Err(e) => {
                eprintln!("Failed to send state commit {}: {}", commit_id, e);
                Err(e)
            }
        }
    }

    pub async fn read_action(&mut self) -> Result<u32, ProtonError> {
        match self.handler.read_action().await {
            Ok(action) => {
                println!("Received action: {}", action);
                Ok(action)
            }
            Err(e) => {
                eprintln!("Failed to read action: {}", e);
                Err(e)
            }
        }
    }
}

// Certificate verifier that accepts any certificate
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
