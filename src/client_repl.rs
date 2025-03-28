use crate::proton::client::ProtonConnection;
use crate::proton::{ProtonClient, IDLE_TIMEOUT};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::history::FileHistory;
use rustyline::validate::{MatchingBracketValidator, Validator};
use rustyline::Helper;
use rustyline::{CompletionType, Config, Context, Editor};
use std::borrow::Cow::{self, Borrowed};
use std::error::Error;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::time::sleep;

// Define available commands for completion
const COMMANDS: &[&str] = &[
    "connect",
    "send_event",
    "commit",
    "read_action",
    "close",
    "sleep",
    "reset",
    "help",
    "exit",
];

// Helper struct for rustyline functionality
struct ReplHelper {
    validator: MatchingBracketValidator,
    hinter: HistoryHinter,
}

impl ReplHelper {
    fn new() -> Self {
        Self {
            validator: MatchingBracketValidator::new(),
            hinter: HistoryHinter {},
        }
    }
}

// Implement completion for commands
impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // Split the input into parts
        let parts: Vec<&str> = line[..pos].split_whitespace().collect();

        // Handle completion for different contexts
        let (start, candidates) = if parts.is_empty() {
            // Empty line - show all commands
            (
                0,
                COMMANDS
                    .iter()
                    .map(|&cmd| Pair {
                        display: cmd.to_string(),
                        replacement: cmd.to_string(),
                    })
                    .collect(),
            )
        } else {
            let last_word = parts.last().unwrap();
            let last_word_start = line[..pos].rfind(last_word).unwrap_or(0);

            // Check if we're completing a number prefix
            if last_word.chars().all(|c| c.is_digit(10)) && pos == line.len() {
                (
                    pos,
                    vec![Pair {
                        display: " connect".to_string(),
                        replacement: " connect".to_string(),
                    }],
                )
            } else {
                // Filter commands that match the current word
                let matches: Vec<Pair> = COMMANDS
                    .iter()
                    .filter(|&cmd| cmd.starts_with(last_word))
                    .map(|&cmd| Pair {
                        display: cmd.to_string(),
                        replacement: cmd.to_string(),
                    })
                    .collect();
                (last_word_start, matches)
            }
        };

        Ok((start, candidates))
    }
}

impl Highlighter for ReplHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Borrowed(hint)
    }
}

impl Hinter for ReplHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        self.hinter.hint(line, pos, ctx)
    }
}

impl Validator for ReplHelper {
    fn validate(
        &self,
        ctx: &mut rustyline::validate::ValidationContext,
    ) -> rustyline::Result<rustyline::validate::ValidationResult> {
        self.validator.validate(ctx)
    }
}

impl Helper for ReplHelper {}

pub struct ClientRepl {
    client: ProtonClient,
    server_addr: SocketAddr,
    connection: Option<ProtonConnection>,
    editor: Editor<ReplHelper, FileHistory>,
}

impl ClientRepl {
    pub fn new(bind_addr: SocketAddr, server_addr: SocketAddr) -> Result<Self, Box<dyn Error>> {
        let client = ProtonClient::new(bind_addr)?;

        // Configure readline
        let config = Config::builder()
            .history_ignore_space(true)
            .completion_type(CompletionType::List)
            .build();

        let mut editor = Editor::with_config(config)?;
        editor.set_helper(Some(ReplHelper::new()));

        // Load history from ~/.proton_history
        if let Some(mut home) = home::home_dir() {
            home.push(".proton_history");
            let _ = editor.load_history(&home);
        }

        Ok(Self {
            client,
            server_addr,
            connection: None,
            editor,
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
        println!("\nRepeat prefix:");
        println!("  Commands can be prefixed with a number to repeat them");
        println!("  Example: 5 connect    - Connects 5 times");
        println!("  Example: 3 send_event - Sends 3 events");
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

    async fn parse_and_handle_command(&mut self, command: &str) -> bool {
        let parts: Vec<&str> = command.trim().splitn(2, ' ').collect();

        // Check if first part is a number (repeat count)
        let (repeat_count, cmd) = if let Ok(count) = parts[0].parse::<u32>() {
            if parts.len() < 2 {
                println!("Error: Repeat count needs a command");
                return true;
            }
            (count, parts[1])
        } else {
            (1, command)
        };

        // Execute the command repeat_count times
        for i in 0..repeat_count {
            if repeat_count > 1 {
                println!("Execution {} of {}:", i + 1, repeat_count);
            }
            if !self.handle_single_command(cmd).await {
                return false;
            }
        }
        true
    }

    async fn handle_command(&mut self, command: &str) -> bool {
        // Split commands by semicolon and handle each one
        for cmd in command.split(';') {
            if !self.parse_and_handle_command(cmd.trim()).await {
                return false; // Exit if any command returns false (i.e., exit command)
            }
        }
        true
    }

    pub async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        println!("Starting REPL client mode...");
        Self::print_help();

        loop {
            let readline = self.editor.readline("> ");
            match readline {
                Ok(line) => {
                    let line = line.trim();
                    if !line.is_empty() {
                        self.editor.add_history_entry(line)?;
                    }

                    if !self.handle_command(line).await {
                        break;
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("^D");
                    break;
                }
                Err(err) => {
                    println!("Error: {}", err);
                    break;
                }
            }
        }

        // Save history
        if let Some(mut home) = home::home_dir() {
            home.push(".proton_history");
            let _ = self.editor.save_history(&home);
        }

        // Cleanup connection if exists
        if let Some(ref mut conn) = self.connection {
            conn.close().await;
        }

        Ok(())
    }
}
