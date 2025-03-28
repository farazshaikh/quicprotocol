use crate::proton::client::ProtonConnection;
use crate::proton::{ProtonClient, IDLE_TIMEOUT};
use std::error::Error;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::time::sleep;

pub struct ClientRepl {
    client: ProtonClient,
    server_addr: SocketAddr,
    connection: Option<ProtonConnection>,
}

impl ClientRepl {
    pub fn new(bind_addr: SocketAddr, server_addr: SocketAddr) -> Result<Self, Box<dyn Error>> {
        let client = ProtonClient::new(bind_addr)?;
        Ok(Self {
            client,
            server_addr,
            connection: None,
        })
    }

    fn print_help() {
        println!("Available commands:");
        println!("  connect [secs]   - Connect to the server with optional startup delay");
        println!("  send_event       - Send an event");
        println!("  commit <id>      - Send a state commit with given ID");
        println!("  read_action      - Read an action from server");
        println!("  close            - Close the connection");
        println!("  sleep <secs>     - Sleep for specified seconds");
        println!("  reset            - Reset client state and wait for connections to timeout");
        println!("  help             - Show this help message");
        println!("  exit             - Exit the REPL");
        println!("\nCommands can be chained with semicolons:");
        println!("  Example: connect 5; sleep 2; send_event; read_action");
        println!("\nConnection handling:");
        println!("  - Multiple connects allowed to test connection handling");
        println!("  - Use 'reset' to cleanup all connections and start fresh");
    }

    async fn handle_single_command(&mut self, command: &str) -> bool {
        match command.trim() {
            "help" => {
                Self::print_help();
                true
            }
            cmd if cmd.starts_with("connect") => {
                // Parse optional delay parameter
                let delay = cmd
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(Duration::from_secs);

                println!(
                    "Connecting to server at {}{}...",
                    self.server_addr,
                    delay
                        .map(|d| format!(" with {}s startup delay", d.as_secs()))
                        .unwrap_or_default()
                );

                // If there's an existing connection, warn but proceed
                if self.connection.is_some() {
                    println!("Warning: Creating new connection while previous connection exists");
                }

                match self.client.connect(self.server_addr, delay).await {
                    Ok(conn) => {
                        println!("Connected successfully!");
                        // Replace any existing connection
                        self.connection = Some(conn);
                    }
                    Err(e) => println!("Failed to connect: {}", e),
                }
                true
            }
            "reset" => {
                // Close any existing connection
                if let Some(ref mut conn) = self.connection {
                    conn.close().await;
                    self.connection = None;
                }

                // Wait for twice the idle timeout to ensure all connections are cleaned up
                let wait_time = IDLE_TIMEOUT.as_secs() * 2;
                println!("Waiting {}s for all connections to timeout...", wait_time);
                sleep(Duration::from_secs(wait_time)).await;
                println!("Reset complete. Client state cleared.");
                true
            }
            "send_event" => {
                if let Some(ref mut conn) = self.connection {
                    match conn.send_event().await {
                        Ok(ack) => println!("Event acknowledged with ID: {}", ack),
                        Err(e) => println!("Failed to send event: {}", e),
                    }
                } else {
                    println!("Not connected! Use 'connect' first.");
                }
                true
            }
            cmd if cmd.starts_with("commit ") => {
                if let Some(ref mut conn) = self.connection {
                    if let Ok(id) = cmd.split_whitespace().nth(1).unwrap_or("0").parse::<u32>() {
                        match conn.send_state_commit(id).await {
                            Ok(response) => println!("State commit response: {}", response),
                            Err(e) => println!("Failed to commit state: {}", e),
                        }
                    } else {
                        println!("Invalid commit ID. Usage: commit <number>");
                    }
                } else {
                    println!("Not connected! Use 'connect' first.");
                }
                true
            }
            cmd if cmd.starts_with("sleep ") => {
                if let Ok(secs) = cmd.split_whitespace().nth(1).unwrap_or("0").parse::<u64>() {
                    println!("Sleeping for {} seconds...", secs);
                    sleep(Duration::from_secs(secs)).await;
                    println!("Awake!");
                } else {
                    println!("Invalid sleep duration. Usage: sleep <seconds>");
                }
                true
            }
            "read_action" => {
                if let Some(ref mut conn) = self.connection {
                    match conn.read_action().await {
                        Ok(action) => println!("Received action: {}", action),
                        Err(e) => println!("Failed to read action: {}", e),
                    }
                } else {
                    println!("Not connected! Use 'connect' first.");
                }
                true
            }
            "close" => {
                if let Some(ref mut conn) = self.connection {
                    conn.close().await;
                    self.connection = None;
                    println!("Connection closed.");
                } else {
                    println!("Not connected!");
                }
                true
            }
            "exit" => {
                if let Some(ref mut conn) = self.connection {
                    conn.close().await;
                }
                println!("Goodbye!");
                false
            }
            "" => true,
            _ => {
                println!("Unknown command. Type 'help' for available commands.");
                true
            }
        }
    }

    async fn handle_command(&mut self, command: &str) -> bool {
        // Split commands by semicolon and handle each one
        for cmd in command.split(';') {
            if !self.handle_single_command(cmd.trim()).await {
                return false; // Exit if any command returns false (i.e., exit command)
            }
        }
        true
    }

    pub async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        println!("Starting REPL client mode...");
        Self::print_help();

        let mut input = String::new();
        loop {
            print!("> ");
            io::stdout().flush()?;
            input.clear();
            io::stdin().read_line(&mut input)?;

            if !self.handle_command(input.trim()).await {
                break;
            }
        }
        Ok(())
    }
}
