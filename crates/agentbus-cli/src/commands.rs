//! agentbus CLI (fr:10, v0.2): a thin wrapper over the spool store.
//! Single results print as pretty JSON; streams print one compact JSON
//! value per line.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::Value;

use agentbus_core::envelope::Kind;
use agentbus_core::store::{EventFilter, RegisterOpts, Store, StoreError, SweepOpts};

#[derive(Parser)]
#[command(
    name = "agentbus",
    version,
    about = "agentbus CLI (daemonless spool store)"
)]
pub struct Cli {
    /// Store directory (default ~/.agentbus).
    #[arg(long, env = "AGENTBUS_DIR")]
    pub dir: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Register an instance id (non-persistent rows die with this process;
    /// pair with --persistent for durable addresses).
    Register {
        id: String,
        #[arg(long)]
        persistent: bool,
        #[arg(long)]
        on_delivery: Option<String>,
    },
    /// Remove a registration (the inbox file is kept).
    Unregister { id: String },
    /// List registered instances.
    Ls,
    /// Send a one-way message (payload from --file or stdin).
    Send {
        to: String,
        #[arg(long, default_value = "ext:cli")]
        from: String,
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Send a request and wait for the reply.
    Ask {
        to: String,
        #[arg(long, default_value = "ext:cli")]
        from: String,
        #[arg(long, default_value_t = 30_000)]
        timeout_ms: u64,
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Fetch the (possibly late) reply to an earlier ask.
    AskResult { request_id: String },
    /// Reply to an ask as <from>.
    Reply {
        request_id: String,
        from: String,
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Drain an instance's inbox without blocking.
    CheckInbox { id: String },
    /// Block until messages arrive, or time out (empty list).
    Await {
        id: String,
        #[arg(long, default_value_t = 30_000)]
        timeout_ms: u64,
    },
    /// Publish a broadcast event.
    Publish {
        #[arg(long, default_value = "ext:cli")]
        from: String,
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Read the event log as {"seq":..,"envelope":..} lines; --follow polls.
    Events {
        #[arg(long)]
        follow: bool,
        #[arg(long, default_value_t = 0)]
        since: i64,
        #[arg(long)]
        instance: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
    /// Stream envelopes addressed to one instance, one compact JSON per
    /// line, never consuming the inbox (spec 6.7; for harness monitor tools).
    Watch {
        id: String,
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
    /// Crash recovery: prune dead registrations, re-fire stale hooks,
    /// report expired asks (spec 6.8).
    Sweep {
        #[arg(long)]
        purge_orphans: bool,
        #[arg(long, default_value_t = 60)]
        grace_secs: u64,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    let mut store = match &cli.dir {
        Some(dir) => Store::open_at(dir)?,
        None => Store::open()?,
    };
    match cli.cmd {
        Cmd::Register {
            id,
            persistent,
            on_delivery,
        } => {
            store.register(
                &id,
                &RegisterOpts {
                    persistent,
                    on_delivery,
                    pid: None,
                },
            )?;
            println!("{}", serde_json::json!({"ok": true}));
        }
        Cmd::Unregister { id } => {
            let removed = store.unregister(&id)?;
            println!("{}", serde_json::json!({ "ok": removed }));
        }
        Cmd::Ls => {
            let rows = store.list_instances()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "instances": rows }))?
            );
        }
        Cmd::Send { to, from, file } => {
            let delivered = store.send(&from, &to, read_payload(&file)?)?;
            warn_on_hook_failure(delivered.hook.as_ref());
            println!("{}", serde_json::json!({"id": delivered.envelope.id}));
        }
        Cmd::Ask {
            to,
            from,
            timeout_ms,
            file,
        } => {
            let payload = read_payload(&file)?;
            match store.ask(&from, &to, payload, Duration::from_millis(timeout_ms)) {
                Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply)?),
                Err(StoreError::Timeout(rid)) => {
                    eprintln!(
                        "error[timeout]: no reply within {timeout_ms} ms; \
                         retrieve a late reply with: agentbus ask-result {rid}"
                    );
                    // Safe to skip Store::drop: the process exits now and
                    // SQLite WAL frames are OS-durable without a close.
                    std::process::exit(2);
                }
                Err(e) => return Err(e.into()),
            }
        }
        Cmd::AskResult { request_id } => {
            let status = store.ask_result(&request_id)?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        Cmd::Reply {
            request_id,
            from,
            file,
        } => {
            store.reply(&from, &request_id, read_payload(&file)?)?;
            println!("{}", serde_json::json!({"ok": true}));
        }
        Cmd::CheckInbox { id } => {
            let envelopes = store.check_inbox(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "envelopes": envelopes }))?
            );
        }
        Cmd::Await { id, timeout_ms } => {
            let envelopes = store.await_message(&id, Duration::from_millis(timeout_ms))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "envelopes": envelopes }))?
            );
        }
        Cmd::Publish { from, file } => {
            let id = store.publish_event(&from, read_payload(&file)?)?;
            println!("{}", serde_json::json!({ "id": id }));
        }
        Cmd::Events {
            follow,
            since,
            instance,
            kind,
            interval_ms,
        } => {
            let filter = EventFilter {
                instance,
                kind: parse_kind(kind.as_deref())?,
                to: None,
            };
            stream_events(
                &store,
                since,
                &filter,
                follow,
                Duration::from_millis(interval_ms),
                true,
            )?;
        }
        Cmd::Watch { id, interval_ms } => {
            let filter = EventFilter {
                to: Some(id),
                ..Default::default()
            };
            let cursor = store.max_seq()?; // start live: no replay (spec 6.7)
            stream_events(
                &store,
                cursor,
                &filter,
                true,
                Duration::from_millis(interval_ms),
                false,
            )?;
        }
        Cmd::Sweep {
            purge_orphans,
            grace_secs,
        } => {
            let report = store.sweep(&SweepOpts {
                grace: Duration::from_secs(grace_secs),
                purge_orphans,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}

fn parse_kind(kind: Option<&str>) -> Result<Option<Kind>> {
    match kind {
        None => Ok(None),
        Some(s) => s.parse::<Kind>().map(Some).map_err(anyhow::Error::msg),
    }
}

/// Drain rows from `since`; with `follow`, poll forever. `with_seq` selects
/// the {"seq":..,"envelope":..} line shape (events) vs bare envelopes (watch).
fn stream_events(
    store: &Store,
    mut cursor: i64,
    filter: &EventFilter,
    follow: bool,
    interval: Duration,
    with_seq: bool,
) -> Result<()> {
    use std::io::Write;
    loop {
        loop {
            let page = store.events_since(cursor, 1000, filter)?;
            let drained = page.events.is_empty() && page.cursor == cursor;
            for ev in &page.events {
                if with_seq {
                    println!("{}", serde_json::to_string(ev)?);
                } else {
                    println!("{}", serde_json::to_string(&ev.envelope)?);
                }
            }
            cursor = page.cursor;
            if drained {
                break;
            }
        }
        std::io::stdout().flush()?;
        if !follow {
            return Ok(());
        }
        std::thread::sleep(interval);
    }
}

fn warn_on_hook_failure(hook: Option<&agentbus_core::store::HookOutcome>) {
    if let Some(h) = hook {
        if !h.ok {
            eprintln!("warning: on_delivery hook failed: {}", h.detail);
        }
    }
}

fn read_payload(file: &Option<String>) -> Result<Value> {
    let raw = match file {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading payload file `{path}`: {e}"))?,
        None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    Ok(serde_json::from_str(&raw)?)
}
