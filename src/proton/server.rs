use crate::proton::{
    ProtonError, IDLE_TIMEOUT, MAX_BIDIRECTIONAL_STREAMS, MAX_CONNECTIONS, STARTUP_DELAY,
    STREAM_ACTION, STREAM_EVENT, STREAM_STATE_COMMIT, STREAM_TIMEOUT,
};
use quinn::{Connection as QuinnConnection, Endpoint, RecvStream, SendStream, ServerConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};

struct StreamPair {
    send: SendStream,
    recv: RecvStream,
}

struct ProtonStreamHandler {
    event_stream: Option<StreamPair>,
    state_commit_stream: Option<StreamPair>,
    action_stream: Option<StreamPair>,
    last_event_id: u32,
}

impl ProtonStreamHandler {
    fn new() -> Self {
        Self {
            event_stream: None,
            state_commit_stream: None,
            action_stream: None,
            last_event_id: 0,
        }
    }

    async fn handle_stream(
        &mut self,
        send: SendStream,
        mut recv: RecvStream,
    ) -> Result<(), ProtonError> {
        let mut discriminator = [0u8; 1];
        timeout(STREAM_TIMEOUT, recv.read_exact(&mut discriminator)).await??;

        match discriminator[0] {
            STREAM_EVENT => {
                if self.event_stream.is_none() {
                    self.event_stream = Some(StreamPair { send, recv });
                    Ok(())
                } else {
                    Err(ProtonError::InvalidStream)
                }
            }
            STREAM_STATE_COMMIT => {
                if self.state_commit_stream.is_none() {
                    self.state_commit_stream = Some(StreamPair { send, recv });
                    Ok(())
                } else {
                    Err(ProtonError::InvalidStream)
                }
            }
            STREAM_ACTION => {
                if self.action_stream.is_none() {
                    self.action_stream = Some(StreamPair { send, recv });
                    Ok(())
                } else {
                    Err(ProtonError::InvalidStream)
                }
            }
            _ => Err(ProtonError::InvalidStream),
        }
    }

    async fn handle_all_streams(
        &mut self,
        connection: &QuinnConnection,
    ) -> Result<(), ProtonError> {
        let closed = connection.closed();

        let event_stream_fut = async {
            if let Some(StreamPair {
                ref mut send,
                ref mut recv,
            }) = self.event_stream
            {
                loop {
                    let mut data = [0u8; 4];
                    match timeout(STREAM_TIMEOUT, recv.read_exact(&mut data)).await {
                        Ok(Ok(_)) => {
                            let event_id = u32::from_le_bytes(data);

                            // Verify monotonicity
                            if event_id <= self.last_event_id {
                                return Err(ProtonError::InvalidStream);
                            }
                            self.last_event_id = event_id;

                            // Send acknowledgment
                            match timeout(STREAM_TIMEOUT, send.write_all(&event_id.to_le_bytes()))
                                .await
                            {
                                Ok(Ok(_)) => {
                                    println!("Event {} acknowledged", event_id);
                                }
                                Ok(Err(e)) => {
                                    eprintln!("Failed to send event ack: {}", e);
                                    return Err(ProtonError::ConnectionError);
                                }
                                Err(_) => {
                                    eprintln!("Timeout sending event ack");
                                    return Err(ProtonError::Timeout);
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            eprintln!("Failed to read event: {}", e);
                            return Err(ProtonError::ConnectionError);
                        }
                        Err(_) => {
                            eprintln!("Timeout reading event");
                            return Err(ProtonError::Timeout);
                        }
                    }
                }
            }
            Ok(())
        };

        let state_commit_stream_fut = async {
            if let Some(StreamPair {
                ref mut send,
                ref mut recv,
            }) = self.state_commit_stream
            {
                loop {
                    let mut data = [0u8; 4];
                    match timeout(STREAM_TIMEOUT, recv.read_exact(&mut data)).await {
                        Ok(Ok(_)) => {
                            let commit_id = u32::from_le_bytes(data);
                            println!("Received state commit: {}", commit_id);

                            // Send response
                            let response = commit_id + 2;
                            match timeout(STREAM_TIMEOUT, send.write_all(&response.to_le_bytes()))
                                .await
                            {
                                Ok(Ok(_)) => {
                                    println!("State commit {} response sent", commit_id);
                                }
                                Ok(Err(e)) => {
                                    eprintln!("Failed to send state commit response: {}", e);
                                    return Err(ProtonError::ConnectionError);
                                }
                                Err(_) => {
                                    eprintln!("Timeout sending state commit response");
                                    return Err(ProtonError::Timeout);
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            eprintln!("Failed to read state commit: {}", e);
                            return Err(ProtonError::ConnectionError);
                        }
                        Err(_) => {
                            eprintln!("Timeout reading state commit");
                            return Err(ProtonError::Timeout);
                        }
                    }
                }
            }
            Ok(())
        };

        let action_stream_fut = async {
            if let Some(StreamPair {
                ref mut send,
                ref mut recv,
            }) = self.action_stream
            {
                let mut counter = 0u32;
                loop {
                    let mut data = [0u8; 4];
                    match timeout(STREAM_TIMEOUT, recv.read_exact(&mut data)).await {
                        Ok(Ok(_)) => {
                            let request_id = u32::from_le_bytes(data);
                            println!("Received action request: {}", request_id);

                            // Send action
                            let action = counter;
                            match timeout(STREAM_TIMEOUT, send.write_all(&action.to_le_bytes()))
                                .await
                            {
                                Ok(Ok(_)) => {
                                    println!("Action {} sent", action);
                                    counter += 1;
                                }
                                Ok(Err(e)) => {
                                    eprintln!("Failed to send action: {}", e);
                                    return Err(ProtonError::ConnectionError);
                                }
                                Err(_) => {
                                    eprintln!("Timeout sending action");
                                    return Err(ProtonError::Timeout);
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            eprintln!("Failed to read action request: {}", e);
                            return Err(ProtonError::ConnectionError);
                        }
                        Err(_) => {
                            eprintln!("Timeout reading action request");
                            return Err(ProtonError::Timeout);
                        }
                    }
                }
            }
            Ok(())
        };

        tokio::select! {
            _ = closed => {
                println!("Client closed connection");
                Ok(())
            }
            r = event_stream_fut => r,
            r = state_commit_stream_fut => r,
            r = action_stream_fut => r,
        }
    }
}

pub struct ProtonServer {
    endpoint: Endpoint,
    active_connection: Arc<Mutex<Option<ProtonStreamHandler>>>,
}

impl ProtonServer {
    pub fn new(
        addr: SocketAddr,
        cert: rustls::Certificate,
        key: rustls::PrivateKey,
    ) -> Result<Self, ProtonError> {
        // Configure TLS
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .map_err(|e| ProtonError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        server_crypto.alpn_protocols = vec![b"proton".to_vec()];

        // Configure QUIC server
        let mut server_config = ServerConfig::with_crypto(Arc::new(server_crypto));
        let mut transport_config = quinn::TransportConfig::default();
        transport_config
            .keep_alive_interval(Some(std::time::Duration::from_secs(5)))
            .max_idle_timeout(Some(IDLE_TIMEOUT.try_into().unwrap()))
            .max_concurrent_bidi_streams(MAX_BIDIRECTIONAL_STREAMS.into());
        server_config.transport_config(Arc::new(transport_config));

        // Only allow one connection
        server_config.concurrent_connections(MAX_CONNECTIONS.into());

        // Create endpoint
        let endpoint = Endpoint::server(server_config, addr)?;

        Ok(ProtonServer {
            endpoint,
            active_connection: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn run(&self) -> Result<(), ProtonError> {
        // Wait for startup delay to ensure old connections are cleaned up
        println!(
            "Waiting {} seconds for startup delay...",
            STARTUP_DELAY.as_secs()
        );
        sleep(STARTUP_DELAY).await;

        println!("Server listening on {}", self.endpoint.local_addr()?);

        // Only accept one connection at a time
        while let Some(connecting) = self.endpoint.accept().await {
            let active_connection = Arc::clone(&self.active_connection);

            // Handle the new connection in a separate task
            let connection_handle = tokio::spawn(async move {
                match Self::handle_connection(connecting, active_connection).await {
                    Ok(_) => println!("Connection handled successfully"),
                    Err(e) => eprintln!("Connection error: {}", e),
                }
            });

            // Wait for this connection to complete before accepting another
            if let Err(e) = connection_handle.await {
                eprintln!("Connection task failed: {}", e);
            }

            // Ensure connection is cleaned up
            *self.active_connection.lock().await = None;
            println!("Connection cleanup complete, ready for new connections");
        }

        Ok(())
    }

    async fn handle_connection(
        connecting: quinn::Connecting,
        active_connection: Arc<Mutex<Option<ProtonStreamHandler>>>,
    ) -> Result<(), ProtonError> {
        let connection = connecting.await?;
        println!(
            "Connection established from {}",
            connection.remote_address()
        );

        // Check if there's already an active connection
        let mut conn_guard = active_connection.lock().await;
        if conn_guard.is_some() {
            println!("Rejecting connection: another client is already connected");
            drop(conn_guard);
            connection.close(0u32.into(), b"Another client is already connected");
            return Err(ProtonError::ConnectionError);
        }

        // Create new stream handler
        let mut stream_handler = ProtonStreamHandler::new();
        let mut streams_established = 0;

        // Accept exactly 3 streams with timeout
        while streams_established < 3 {
            match timeout(std::time::Duration::from_secs(5), connection.accept_bi()).await {
                Ok(Ok((send, recv))) => {
                    if let Err(e) = stream_handler.handle_stream(send, recv).await {
                        println!("Error handling stream: {}", e);
                        *conn_guard = None;
                        connection.close(1u32.into(), b"Stream setup error");
                        return Err(e);
                    }
                    streams_established += 1;
                    println!("Stream {} established", streams_established);
                }
                Ok(Err(e)) => {
                    println!("Error accepting stream: {}", e);
                    *conn_guard = None;
                    connection.close(2u32.into(), b"Stream accept error");
                    return Err(ProtonError::ConnectionError);
                }
                Err(_) => {
                    println!("Timeout waiting for stream establishment");
                    *conn_guard = None;
                    connection.close(3u32.into(), b"Stream setup timeout");
                    return Err(ProtonError::ConnectionError);
                }
            }
        }

        // Store the active connection
        *conn_guard = Some(stream_handler);
        let mut handler = conn_guard.take().unwrap();
        // Drop the lock so we can acquire it again later
        drop(conn_guard);

        // Handle all streams in a single task
        let stream_result = handler.handle_all_streams(&connection).await;

        // Get the lock again to clear the connection state
        let mut conn_guard = active_connection.lock().await;
        *conn_guard = None;
        drop(conn_guard);
        println!("Connection state cleared");

        // Handle the stream result and close the connection appropriately
        match stream_result {
            Ok(_) => {
                println!("Streams completed normally");
                connection.close(0u32.into(), b"Streams completed");
            }
            Err(ProtonError::Timeout) => {
                eprintln!("Stream operation timed out");
                connection.close(4u32.into(), b"Stream operation timeout");
            }
            Err(e) => {
                eprintln!("Stream error: {}", e);
                connection.close(5u32.into(), b"Stream error");
            }
        }

        Ok(())
    }
}
