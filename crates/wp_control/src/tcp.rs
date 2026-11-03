//! Line-delimited JSON-RPC over TCP, for driving a live windowed game.
//! Binds to 127.0.0.1 only: the endpoint can change the world, so it must
//! never be reachable from the network.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc::{Receiver, Sender, channel};
use wp_core::Diagnostic;

/// One request line waiting for the game loop. Send the response line (or
/// `None` for a notification) on `reply`.
pub struct Pending {
    pub line: String,
    pub reply: Sender<Option<String>>,
}

/// Start listening on `127.0.0.1:port` (0 picks a free port). Requests arrive
/// on the returned channel; the caller handles them between frames.
pub fn listen(port: u16) -> Result<(Receiver<Pending>, SocketAddr), Diagnostic> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
        Diagnostic::error("WP-K010", format!("cannot listen on 127.0.0.1:{port}: {e}"))
            .help("pick another port with --control <port>, or 0 for any free port")
    })?;
    let addr = listener
        .local_addr()
        .map_err(|e| Diagnostic::error("WP-K010", e.to_string()))?;
    let (tx, rx) = channel::<Pending>();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let Ok(mut writer) = stream.try_clone() else {
                    return;
                };
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let (reply_tx, reply_rx) = channel();
                    if tx
                        .send(Pending {
                            line,
                            reply: reply_tx,
                        })
                        .is_err()
                    {
                        return;
                    }
                    match reply_rx.recv() {
                        Ok(Some(resp)) => {
                            if writeln!(writer, "{resp}").is_err() {
                                return;
                            }
                        }
                        Ok(None) => {}
                        Err(_) => return,
                    }
                }
            });
        }
    });
    Ok((rx, addr))
}
