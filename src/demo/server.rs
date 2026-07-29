use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

use crate::cli::DemoScenario;
use crate::error::{AppError, Result};

use super::scenario::seed_messages;

const UID_VALIDITY: u64 = 44;

#[derive(Clone)]
struct StoredMessage {
    uid: u64,
    raw_eml: Vec<u8>,
    flags: BTreeSet<String>,
}

struct Mailbox {
    next_uid: u64,
    messages: Vec<StoredMessage>,
}

struct DemoState {
    username: String,
    password_secret: String,
    mailboxes: BTreeMap<String, Mailbox>,
    smtp_messages: Vec<Vec<u8>>,
}

impl DemoState {
    fn seeded(scenario: DemoScenario, username: &str, password_secret: &str) -> Result<Self> {
        let mut state = Self {
            username: username.to_string(),
            password_secret: password_secret.to_string(),
            mailboxes: BTreeMap::new(),
            smtp_messages: Vec::new(),
        };
        for name in ["INBOX", "Sent", "Archive", "Junk", "Trash", "Drafts"] {
            state.ensure_mailbox(name);
        }
        for seed in seed_messages(scenario)? {
            state
                .append(seed.mailbox_name, seed.raw_eml, BTreeSet::new())
                .map_err(|error| AppError::new("demo_fixture_invalid", error))?;
        }
        Ok(state)
    }

    fn ensure_mailbox(&mut self, name: &str) {
        let key = mailbox_key(name);
        self.mailboxes.entry(key).or_insert_with(|| Mailbox {
            next_uid: 1,
            messages: Vec::new(),
        });
    }

    fn mailbox(&self, name: &str) -> std::result::Result<&Mailbox, String> {
        self.mailboxes
            .get(&mailbox_key(name))
            .ok_or_else(|| format!("mailbox not found: {name}"))
    }

    fn mailbox_mut(&mut self, name: &str) -> std::result::Result<&mut Mailbox, String> {
        self.mailboxes
            .get_mut(&mailbox_key(name))
            .ok_or_else(|| format!("mailbox not found: {name}"))
    }

    fn append(
        &mut self,
        mailbox_name: &str,
        raw_eml: Vec<u8>,
        flags: BTreeSet<String>,
    ) -> std::result::Result<u64, String> {
        self.ensure_mailbox(mailbox_name);
        let mailbox = self.mailbox_mut(mailbox_name)?;
        let uid = mailbox.next_uid;
        mailbox.next_uid = mailbox.next_uid.saturating_add(1);
        mailbox.messages.push(StoredMessage {
            uid,
            raw_eml,
            flags,
        });
        Ok(uid)
    }

    fn move_message(
        &mut self,
        source_name: &str,
        uid: u64,
        target_name: &str,
    ) -> std::result::Result<u64, String> {
        let source = self.mailbox_mut(source_name)?;
        let index = source
            .messages
            .iter()
            .position(|message| message.uid == uid)
            .ok_or_else(|| format!("UID {uid} not found in {source_name}"))?;
        let message = source.messages.remove(index);
        self.append(target_name, message.raw_eml, message.flags)
    }

    fn copy_message(
        &mut self,
        source_name: &str,
        uid: u64,
        target_name: &str,
    ) -> std::result::Result<u64, String> {
        let message = self
            .mailbox(source_name)?
            .messages
            .iter()
            .find(|message| message.uid == uid)
            .cloned()
            .ok_or_else(|| format!("UID {uid} not found in {source_name}"))?;
        self.append(target_name, message.raw_eml, message.flags)
    }
}

pub(super) struct RunningDemoServers {
    pub(super) imap_address: SocketAddr,
    pub(super) smtp_address: SocketAddr,
    stop: Arc<AtomicBool>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl RunningDemoServers {
    pub(super) fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}

pub(super) fn ports_are_bindable(imap_port: u16, smtp_port: u16) -> Result<()> {
    if imap_port == 0 || smtp_port == 0 {
        return Err(AppError::new(
            "invalid_request",
            "demo ports must be non-zero because demo init and demo serve run in separate processes",
        ));
    }
    if imap_port == smtp_port {
        return Err(AppError::new(
            "invalid_request",
            "--imap-port and --smtp-port must be different",
        ));
    }
    let imap = bind_listener("IMAP", imap_port, "--imap-port")?;
    let smtp = bind_listener("SMTP", smtp_port, "--smtp-port")?;
    drop((imap, smtp));
    Ok(())
}

pub(super) fn start(
    imap_port: u16,
    smtp_port: u16,
    scenario: DemoScenario,
    username: &str,
    password_secret: &str,
    verbose: bool,
    events: UnboundedSender<Value>,
) -> Result<RunningDemoServers> {
    let imap_listener = bind_listener("IMAP", imap_port, "--imap-port")?;
    let smtp_listener = bind_listener("SMTP", smtp_port, "--smtp-port")?;
    let imap_address = imap_listener
        .local_addr()
        .map_err(|error| AppError::io("read demo IMAP address", &error))?;
    let smtp_address = smtp_listener
        .local_addr()
        .map_err(|error| AppError::io("read demo SMTP address", &error))?;
    let state = Arc::new(Mutex::new(DemoState::seeded(
        scenario,
        username,
        password_secret,
    )?));
    let stop = Arc::new(AtomicBool::new(false));

    let imap_state = Arc::clone(&state);
    let imap_stop = Arc::clone(&stop);
    let imap_events = events.clone();
    let imap_handle = thread::spawn(move || {
        accept_imap(imap_listener, imap_state, imap_stop, imap_events, verbose);
    });

    let smtp_state = Arc::clone(&state);
    let smtp_stop = Arc::clone(&stop);
    let smtp_handle = thread::spawn(move || {
        accept_smtp(smtp_listener, smtp_state, smtp_stop, events, verbose);
    });

    Ok(RunningDemoServers {
        imap_address,
        smtp_address,
        stop,
        handles: vec![imap_handle, smtp_handle],
    })
}

fn bind_listener(protocol: &str, port: u16, recovery_flag: &str) -> Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|error| {
        AppError::retryable(
            "demo_bind_failed",
            format!("could not bind demo {protocol} server on 127.0.0.1:{port}: {error}"),
        )
        .with_hint(format!(
            "Choose an unused port with {recovery_flag} when running `afmail demo init`."
        ))
        .with_details(json!({
            "protocol": protocol.to_ascii_lowercase(),
            "bind_host": "127.0.0.1",
            "port": port,
            "recovery_flag": recovery_flag,
        }))
    })
}

fn accept_imap(
    listener: TcpListener,
    state: Arc<Mutex<DemoState>>,
    stop: Arc<AtomicBool>,
    events: UnboundedSender<Value>,
    verbose: bool,
) {
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = stream.set_nonblocking(false) {
                    emit_server_error(&events, "imap", "configure_connection", &error.to_string());
                    continue;
                }
                if let Err(error) = serve_imap_connection(stream, &state, &stop, &events, verbose) {
                    emit_server_error(&events, "imap", "connection", &error.to_string());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                emit_server_error(&events, "imap", "accept", &error.to_string());
                return;
            }
        }
    }
}

fn accept_smtp(
    listener: TcpListener,
    state: Arc<Mutex<DemoState>>,
    stop: Arc<AtomicBool>,
    events: UnboundedSender<Value>,
    verbose: bool,
) {
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = stream.set_nonblocking(false) {
                    emit_server_error(&events, "smtp", "configure_connection", &error.to_string());
                    continue;
                }
                if let Err(error) = serve_smtp_connection(stream, &state, &stop, &events, verbose) {
                    emit_server_error(&events, "smtp", "connection", &error.to_string());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                emit_server_error(&events, "smtp", "accept", &error.to_string());
                return;
            }
        }
    }
}

fn serve_imap_connection(
    mut writer: TcpStream,
    state: &Arc<Mutex<DemoState>>,
    stop: &Arc<AtomicBool>,
    events: &UnboundedSender<Value>,
    verbose: bool,
) -> std::io::Result<()> {
    writer.set_read_timeout(Some(Duration::from_millis(500)))?;
    writer.set_write_timeout(Some(Duration::from_secs(5)))?;
    let reader_stream = writer.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    write_imap(
        &mut writer,
        b"* OK [CAPABILITY IMAP4rev1 MOVE UIDPLUS] afmail demo ready\r\n",
    )?;
    let mut authenticated = false;
    let mut selected_mailbox = "INBOX".to_string();
    while !stop.load(Ordering::Relaxed) {
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
        emit_raw(events, "imap", &line, verbose);
        let tokens = imap_tokens(&line);
        if tokens.len() < 2 {
            continue;
        }
        let tag = &tokens[0];
        let mut name_index = 1usize;
        let mut name = tokens[name_index].to_ascii_uppercase();
        if name == "UID" && tokens.len() > 2 {
            name_index += 1;
            name = tokens[name_index].to_ascii_uppercase();
        }
        if name == "CAPABILITY" {
            write_imap(
                &mut writer,
                format!("* CAPABILITY IMAP4rev1 MOVE UIDPLUS\r\n{tag} OK CAPABILITY completed\r\n")
                    .as_bytes(),
            )?;
            emit_operation(events, "imap", "capability", json!({}));
            continue;
        }
        if name == "LOGIN" {
            let valid = tokens
                .get(2)
                .zip(tokens.get(3))
                .is_some_and(|(username, password)| {
                    state.lock().ok().is_some_and(|state| {
                        username == &state.username && password == &state.password_secret
                    })
                });
            if valid {
                authenticated = true;
                write_imap(
                    &mut writer,
                    format!("{tag} OK LOGIN completed\r\n").as_bytes(),
                )?;
                emit_operation(events, "imap", "login", json!({"authenticated": true}));
            } else {
                write_imap(
                    &mut writer,
                    format!("{tag} NO authentication failed\r\n").as_bytes(),
                )?;
                emit_operation(events, "imap", "login", json!({"authenticated": false}));
            }
            continue;
        }
        if !authenticated {
            write_imap(
                &mut writer,
                format!("{tag} NO authenticate first\r\n").as_bytes(),
            )?;
            continue;
        }
        if name == "APPEND" {
            handle_imap_append(&mut writer, &mut reader, state, events, tag, &tokens, &line)?;
            continue;
        }
        match name.as_str() {
            "LIST" => {
                let response = format!(
                    "* LIST (\\Noinferiors) \"/\" \"INBOX\"\r\n* LIST (\\Sent) \"/\" \"Sent\"\r\n* LIST (\\Drafts) \"/\" \"Drafts\"\r\n* LIST (\\Archive) \"/\" \"Archive\"\r\n* LIST (\\Junk) \"/\" \"Junk\"\r\n* LIST (\\Trash) \"/\" \"Trash\"\r\n{tag} OK LIST completed\r\n"
                );
                write_imap(&mut writer, response.as_bytes())?;
                emit_operation(events, "imap", "list", json!({"mailbox_count": 6}));
            }
            "CREATE" => {
                let mailbox = tokens.get(name_index + 1).cloned().unwrap_or_default();
                if let Ok(mut state) = state.lock() {
                    state.ensure_mailbox(&mailbox);
                }
                write_imap(
                    &mut writer,
                    format!("{tag} OK CREATE completed\r\n").as_bytes(),
                )?;
                emit_operation(events, "imap", "create", json!({"mailbox_name": mailbox}));
            }
            "SELECT" | "EXAMINE" => {
                selected_mailbox = tokens
                    .get(name_index + 1)
                    .cloned()
                    .unwrap_or_else(|| "INBOX".to_string());
                let (exists, next_uid) = state
                    .lock()
                    .ok()
                    .and_then(|state| {
                        state
                            .mailbox(&selected_mailbox)
                            .ok()
                            .map(|mailbox| (mailbox.messages.len(), mailbox.next_uid))
                    })
                    .unwrap_or((0, 1));
                let response = format!(
                    "* FLAGS (\\Answered \\Deleted \\Draft \\Flagged \\Seen)\r\n* {exists} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY {UID_VALIDITY}] UIDs valid\r\n* OK [UIDNEXT {next_uid}] Next UID\r\n{tag} OK [READ-WRITE] {name} completed\r\n"
                );
                write_imap(&mut writer, response.as_bytes())?;
                emit_operation(
                    events,
                    "imap",
                    &name.to_ascii_lowercase(),
                    json!({
                        "mailbox_name": selected_mailbox,
                        "message_count": exists,
                    }),
                );
            }
            "SEARCH" => {
                let uids = search_uids(state, &selected_mailbox, &tokens);
                let uid_text = uids
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ");
                write_imap(
                    &mut writer,
                    format!("* SEARCH {uid_text}\r\n{tag} OK SEARCH completed\r\n").as_bytes(),
                )?;
                emit_operation(
                    events,
                    "imap",
                    "search",
                    json!({
                        "mailbox_name": selected_mailbox,
                        "matched_message_count": uids.len(),
                    }),
                );
            }
            "FETCH" => {
                let sequence = tokens
                    .get(name_index + 1)
                    .map(String::as_str)
                    .unwrap_or("1:*");
                let fetched =
                    write_imap_fetch(&mut writer, tag, state, &selected_mailbox, sequence, &line)?;
                emit_operation(
                    events,
                    "imap",
                    "fetch",
                    json!({
                        "mailbox_name": selected_mailbox,
                        "message_count": fetched,
                    }),
                );
            }
            "STORE" => {
                let uid = tokens
                    .get(name_index + 1)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                update_flags(state, &selected_mailbox, uid, &line);
                write_imap(
                    &mut writer,
                    format!("{tag} OK STORE completed\r\n").as_bytes(),
                )?;
                emit_operation(
                    events,
                    "imap",
                    "store",
                    json!({"mailbox_name": selected_mailbox, "uid": uid}),
                );
            }
            "MOVE" | "COPY" => {
                let uid = tokens
                    .get(name_index + 1)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                let target = tokens.get(name_index + 2).cloned().unwrap_or_default();
                let target_uid = state.lock().ok().and_then(|mut state| {
                    if name == "MOVE" {
                        state.move_message(&selected_mailbox, uid, &target).ok()
                    } else {
                        state.copy_message(&selected_mailbox, uid, &target).ok()
                    }
                });
                match target_uid {
                    Some(target_uid) => {
                        write_imap(
                            &mut writer,
                            format!(
                                "{tag} OK [COPYUID {UID_VALIDITY} {uid} {target_uid}] {name} completed\r\n"
                            )
                            .as_bytes(),
                        )?;
                        emit_operation(
                            events,
                            "imap",
                            &name.to_ascii_lowercase(),
                            json!({
                                "source_mailbox_name": selected_mailbox,
                                "target_mailbox_name": target,
                                "uid": uid,
                                "target_uid": target_uid,
                            }),
                        );
                    }
                    None => {
                        write_imap(
                            &mut writer,
                            format!("{tag} NO message not found\r\n").as_bytes(),
                        )?;
                    }
                }
            }
            "EXPUNGE" => {
                let removed = expunge_deleted(state, &selected_mailbox);
                for sequence in (1..=removed).rev() {
                    write_imap(&mut writer, format!("* {sequence} EXPUNGE\r\n").as_bytes())?;
                }
                write_imap(
                    &mut writer,
                    format!("{tag} OK EXPUNGE completed\r\n").as_bytes(),
                )?;
                emit_operation(
                    events,
                    "imap",
                    "expunge",
                    json!({
                        "mailbox_name": selected_mailbox,
                        "message_count": removed,
                    }),
                );
            }
            "LOGOUT" => {
                write_imap(
                    &mut writer,
                    format!("* BYE Logging out\r\n{tag} OK LOGOUT completed\r\n").as_bytes(),
                )?;
                emit_operation(events, "imap", "logout", json!({}));
                return Ok(());
            }
            _ => {
                write_imap(
                    &mut writer,
                    format!("{tag} BAD unsupported command\r\n").as_bytes(),
                )?;
            }
        }
    }
    Ok(())
}

fn handle_imap_append(
    writer: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    state: &Arc<Mutex<DemoState>>,
    events: &UnboundedSender<Value>,
    tag: &str,
    tokens: &[String],
    line: &[u8],
) -> std::io::Result<()> {
    let mailbox = tokens.get(2).cloned().unwrap_or_default();
    let draft = String::from_utf8_lossy(line)
        .to_ascii_uppercase()
        .contains("\\DRAFT");
    let literal_bytes = literal_len(line).unwrap_or(0);
    write_imap(writer, b"+ continue\r\n")?;
    let mut raw_eml = vec![0_u8; literal_bytes];
    reader.read_exact(&mut raw_eml)?;
    let mut trailing = [0_u8; 2];
    reader.read_exact(&mut trailing)?;
    let mut flags = BTreeSet::new();
    if draft {
        flags.insert("\\Draft".to_string());
    }
    let uid = state
        .lock()
        .ok()
        .and_then(|mut state| state.append(&mailbox, raw_eml, flags).ok())
        .unwrap_or(0);
    write_imap(
        writer,
        format!("{tag} OK [APPENDUID {UID_VALIDITY} {uid}] APPEND completed\r\n").as_bytes(),
    )?;
    emit_operation(
        events,
        "imap",
        "append",
        json!({
            "mailbox_name": mailbox,
            "message_bytes": literal_bytes,
            "uid": uid,
            "draft": draft,
        }),
    );
    Ok(())
}

fn write_imap_fetch(
    writer: &mut TcpStream,
    tag: &str,
    state: &Arc<Mutex<DemoState>>,
    mailbox_name: &str,
    sequence: &str,
    line: &[u8],
) -> std::io::Result<usize> {
    let query = String::from_utf8_lossy(line).to_ascii_uppercase();
    let includes_body = query.contains("BODY.PEEK[");
    let header_only = query.contains("BODY.PEEK[HEADER");
    let partial = partial_body_range(&query);
    let messages = state
        .lock()
        .ok()
        .and_then(|state| {
            state
                .mailbox(mailbox_name)
                .ok()
                .map(|mailbox| mailbox.messages.clone())
        })
        .unwrap_or_default();
    let mut fetched = 0usize;
    for (index, message) in messages.iter().enumerate() {
        if !uid_sequence_matches(sequence, message.uid) {
            continue;
        }
        fetched += 1;
        let flags = message.flags.iter().cloned().collect::<Vec<_>>().join(" ");
        write!(
            writer,
            "* {} FETCH (UID {} FLAGS ({flags})",
            index + 1,
            message.uid
        )?;
        if query.contains("RFC822.SIZE") {
            write!(writer, " RFC822.SIZE {}", message.raw_eml.len())?;
        }
        if includes_body {
            let source = if header_only {
                message_header(&message.raw_eml)
            } else {
                message.raw_eml.as_slice()
            };
            let body = match partial {
                Some((start, length)) => {
                    let start = start.min(source.len());
                    let end = start.saturating_add(length).min(source.len());
                    &source[start..end]
                }
                None => source,
            };
            let body_key = if header_only {
                "BODY[HEADER]"
            } else {
                "BODY[]"
            };
            write!(writer, " {body_key} {{{}}}\r\n", body.len())?;
            writer.write_all(body)?;
        }
        writer.write_all(b")\r\n")?;
    }
    write!(writer, "{tag} OK FETCH completed\r\n")?;
    writer.flush()?;
    Ok(fetched)
}

fn search_uids(state: &Arc<Mutex<DemoState>>, mailbox_name: &str, tokens: &[String]) -> Vec<u64> {
    let Ok(state) = state.lock() else {
        return Vec::new();
    };
    let Ok(mailbox) = state.mailbox(mailbox_name) else {
        return Vec::new();
    };
    let header_index = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case("MESSAGE-ID"));
    let target = header_index
        .and_then(|index| tokens.get(index + 1))
        .map(|value| value.trim().to_ascii_lowercase());
    mailbox
        .messages
        .iter()
        .filter(|message| {
            target.as_ref().is_none_or(|target| {
                String::from_utf8_lossy(message_header(&message.raw_eml))
                    .to_ascii_lowercase()
                    .contains(target)
            })
        })
        .map(|message| message.uid)
        .collect()
}

fn update_flags(state: &Arc<Mutex<DemoState>>, mailbox_name: &str, uid: u64, line: &[u8]) {
    let text = String::from_utf8_lossy(line);
    let upper = text.to_ascii_uppercase();
    let add = upper.contains("+FLAGS");
    let remove = upper.contains("-FLAGS");
    let flags = text
        .rsplit_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(flags, _)| {
            flags
                .split_whitespace()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Ok(mut state) = state.lock()
        && let Ok(mailbox) = state.mailbox_mut(mailbox_name)
        && let Some(message) = mailbox
            .messages
            .iter_mut()
            .find(|message| message.uid == uid)
    {
        for flag in flags {
            if add {
                message.flags.insert(flag);
            } else if remove {
                message.flags.remove(&flag);
            }
        }
    }
}

fn expunge_deleted(state: &Arc<Mutex<DemoState>>, mailbox_name: &str) -> usize {
    let Ok(mut state) = state.lock() else {
        return 0;
    };
    let Ok(mailbox) = state.mailbox_mut(mailbox_name) else {
        return 0;
    };
    let before = mailbox.messages.len();
    mailbox
        .messages
        .retain(|message| !message.flags.contains("\\Deleted"));
    before.saturating_sub(mailbox.messages.len())
}

fn serve_smtp_connection(
    mut writer: TcpStream,
    state: &Arc<Mutex<DemoState>>,
    stop: &Arc<AtomicBool>,
    events: &UnboundedSender<Value>,
    verbose: bool,
) -> std::io::Result<()> {
    writer.set_read_timeout(Some(Duration::from_millis(500)))?;
    writer.set_write_timeout(Some(Duration::from_secs(5)))?;
    let reader_stream = writer.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    write_smtp(&mut writer, b"220 localhost afmail demo SMTP ready\r\n")?;
    let mut authenticated = false;
    let mut envelope_from = String::new();
    let mut recipient_emails = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
        emit_raw(events, "smtp", line.as_bytes(), verbose);
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            write_smtp(
                &mut writer,
                b"250-localhost\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 52428800\r\n",
            )?;
            emit_operation(events, "smtp", "ehlo", json!({}));
        } else if upper.starts_with("AUTH PLAIN") {
            authenticated = smtp_auth_plain(&line, state);
            write_smtp_auth_result(&mut writer, authenticated)?;
            emit_operation(
                events,
                "smtp",
                "authenticate",
                json!({"authenticated": authenticated}),
            );
        } else if upper.trim() == "AUTH LOGIN" {
            authenticated = smtp_auth_login(&mut writer, &mut reader, state, events, verbose)?;
            write_smtp_auth_result(&mut writer, authenticated)?;
            emit_operation(
                events,
                "smtp",
                "authenticate",
                json!({"authenticated": authenticated}),
            );
        } else if upper.starts_with("MAIL FROM") {
            if !authenticated {
                write_smtp(&mut writer, b"530 Authentication required\r\n")?;
                continue;
            }
            envelope_from = smtp_path(&line);
            recipient_emails.clear();
            write_smtp(&mut writer, b"250 OK\r\n")?;
        } else if upper.starts_with("RCPT TO") {
            recipient_emails.push(smtp_path(&line));
            write_smtp(&mut writer, b"250 OK\r\n")?;
        } else if upper.trim() == "DATA" {
            write_smtp(&mut writer, b"354 End data with <CR><LF>.<CR><LF>\r\n")?;
            let data = read_smtp_data(&mut reader)?;
            let message_bytes = data.len();
            if let Ok(mut state) = state.lock() {
                state.smtp_messages.push(data);
            }
            write_smtp(&mut writer, b"250 queued\r\n")?;
            emit_operation(
                events,
                "smtp",
                "data",
                json!({
                    "envelope_from": envelope_from,
                    "recipient_emails": recipient_emails,
                    "message_bytes": message_bytes,
                    "accepted": true,
                }),
            );
        } else if upper.trim() == "RSET" {
            envelope_from.clear();
            recipient_emails.clear();
            write_smtp(&mut writer, b"250 OK\r\n")?;
        } else if upper.trim() == "NOOP" {
            write_smtp(&mut writer, b"250 OK\r\n")?;
            emit_operation(events, "smtp", "noop", json!({}));
        } else if upper.trim() == "QUIT" {
            write_smtp(&mut writer, b"221 bye\r\n")?;
            emit_operation(events, "smtp", "quit", json!({}));
            return Ok(());
        } else {
            write_smtp(&mut writer, b"250 OK\r\n")?;
        }
    }
    Ok(())
}

fn smtp_auth_plain(line: &str, state: &Arc<Mutex<DemoState>>) -> bool {
    let encoded = line.split_whitespace().nth(2).unwrap_or_default();
    let Ok(decoded) = BASE64_STANDARD.decode(encoded) else {
        return false;
    };
    let fields = decoded.split(|byte| *byte == 0).collect::<Vec<_>>();
    let Some(username) = fields.get(fields.len().saturating_sub(2)) else {
        return false;
    };
    let Some(password) = fields.last() else {
        return false;
    };
    credentials_match(state, username, password)
}

fn smtp_auth_login(
    writer: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    state: &Arc<Mutex<DemoState>>,
    events: &UnboundedSender<Value>,
    verbose: bool,
) -> std::io::Result<bool> {
    write_smtp(writer, b"334 VXNlcm5hbWU6\r\n")?;
    let mut username = String::new();
    reader.read_line(&mut username)?;
    emit_raw_secret(events, "smtp", "AUTH LOGIN username", verbose);
    write_smtp(writer, b"334 UGFzc3dvcmQ6\r\n")?;
    let mut password = String::new();
    reader.read_line(&mut password)?;
    emit_raw_secret(events, "smtp", "AUTH LOGIN password", verbose);
    let username = BASE64_STANDARD.decode(username.trim()).unwrap_or_default();
    let password = BASE64_STANDARD.decode(password.trim()).unwrap_or_default();
    Ok(credentials_match(state, &username, &password))
}

fn credentials_match(state: &Arc<Mutex<DemoState>>, username: &[u8], password: &[u8]) -> bool {
    state.lock().ok().is_some_and(|state| {
        username == state.username.as_bytes() && password == state.password_secret.as_bytes()
    })
}

fn write_smtp_auth_result(writer: &mut TcpStream, authenticated: bool) -> std::io::Result<()> {
    if authenticated {
        write_smtp(writer, b"235 Authentication successful\r\n")
    } else {
        write_smtp(writer, b"535 Authentication failed\r\n")
    }
}

fn read_smtp_data(reader: &mut BufReader<TcpStream>) -> std::io::Result<Vec<u8>> {
    let mut data = Vec::new();
    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 || line == b".\r\n" || line == b".\n" {
            break;
        }
        if line.starts_with(b"..") {
            line.remove(0);
        }
        data.extend_from_slice(&line);
    }
    Ok(data)
}

fn smtp_path(line: &str) -> String {
    line.split_once(':')
        .map(|(_, path)| {
            path.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
        .unwrap_or_default()
}

fn emit_operation(events: &UnboundedSender<Value>, protocol: &str, operation: &str, fields: Value) {
    let mut payload = json!({
        "event": "demo_mail_operation",
        "level": "info",
        "message": format!("{} {}", protocol.to_ascii_uppercase(), operation.to_ascii_uppercase()),
        "protocol": protocol,
        "operation": operation,
    });
    if let (Value::Object(payload), Value::Object(fields)) = (&mut payload, fields) {
        payload.extend(fields);
    }
    let _ = events.send(payload);
}

fn emit_server_error(
    events: &UnboundedSender<Value>,
    protocol: &str,
    operation: &str,
    error: &str,
) {
    let _ = events.send(json!({
        "event": "demo_mail_server_error",
        "level": "error",
        "message": format!("demo {protocol} {operation} failed"),
        "protocol": protocol,
        "operation": operation,
        "error": error,
    }));
}

fn emit_raw(events: &UnboundedSender<Value>, protocol: &str, line: &[u8], verbose: bool) {
    if !verbose {
        return;
    }
    let text = String::from_utf8_lossy(line).trim().to_string();
    let upper = text.to_ascii_uppercase();
    let redacted = if upper.contains(" LOGIN ") {
        text.split_whitespace()
            .next()
            .map(|tag| format!("{tag} LOGIN *** ***"))
            .unwrap_or_else(|| "LOGIN *** ***".to_string())
    } else if upper.starts_with("AUTH ") {
        "AUTH ***".to_string()
    } else {
        text
    };
    let _ = events.send(json!({
        "event": "demo_raw_protocol",
        "level": "debug",
        "message": redacted,
        "protocol": protocol,
        "direction": "client_to_server",
    }));
}

fn emit_raw_secret(events: &UnboundedSender<Value>, protocol: &str, label: &str, verbose: bool) {
    if !verbose {
        return;
    }
    let _ = events.send(json!({
        "event": "demo_raw_protocol",
        "level": "debug",
        "message": format!("{label} ***"),
        "protocol": protocol,
        "direction": "client_to_server",
    }));
}

fn imap_tokens(line: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(line);
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in text.trim().chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' && quoted {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character.is_whitespace() && !quoted {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn literal_len(line: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(line);
    let (_, suffix) = text.rsplit_once('{')?;
    let (digits, _) = suffix.split_once('}')?;
    digits.trim_end_matches('+').parse().ok()
}

fn uid_sequence_matches(sequence: &str, uid: u64) -> bool {
    sequence.split(',').any(|part| {
        if let Some((start, end)) = part.split_once(':') {
            let start = start.parse::<u64>().unwrap_or(1);
            end == "*" && uid >= start
                || end
                    .parse::<u64>()
                    .is_ok_and(|end| uid >= start && uid <= end)
        } else {
            part.parse::<u64>().is_ok_and(|candidate| candidate == uid)
        }
    })
}

fn partial_body_range(query: &str) -> Option<(usize, usize)> {
    let marker = "BODY.PEEK[]<";
    let start = query.find(marker)? + marker.len();
    let rest = query.get(start..)?;
    let (range, _) = rest.split_once('>')?;
    let (offset, length) = range.split_once('.')?;
    Some((offset.parse().ok()?, length.parse().ok()?))
}

fn message_header(raw: &[u8]) -> &[u8] {
    raw.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| &raw[..index + 4])
        .or_else(|| {
            raw.windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| &raw[..index + 2])
        })
        .unwrap_or(raw)
}

fn mailbox_key(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn write_imap(writer: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    writer.write_all(bytes)?;
    writer.flush()
}

fn write_smtp(writer: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    writer.write_all(bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::{imap_tokens, literal_len, partial_body_range, start, uid_sequence_matches};
    use crate::cli::DemoScenario;
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpStream;
    use tokio::sync::mpsc;

    #[test]
    fn imap_parser_preserves_quoted_credentials_and_mailbox_names() {
        assert_eq!(
            imap_tokens(b"a1 LOGIN \"founder\" \"random secret\"\r\n"),
            ["a1", "LOGIN", "founder", "random secret"]
        );
        assert_eq!(
            imap_tokens(b"a2 UID MOVE 7 \"Sent Mail\"\r\n"),
            ["a2", "UID", "MOVE", "7", "Sent Mail"]
        );
        assert_eq!(
            literal_len(b"a3 APPEND \"Drafts\" (\\Draft) {42}\r\n"),
            Some(42)
        );
    }

    #[test]
    fn uid_sequences_and_partial_ranges_match_client_queries() {
        assert!(uid_sequence_matches("1:*", 12));
        assert!(uid_sequence_matches("1,3,9:11", 10));
        assert!(!uid_sequence_matches("1,3,9:11", 8));
        assert_eq!(
            partial_body_range("A UID FETCH 1 (BODY.PEEK[]<1048576.42>)"),
            Some((1_048_576, 42))
        );
    }

    #[test]
    fn demo_imap_accepts_only_the_configured_credential() {
        let (events, _event_rx) = mpsc::unbounded_channel();
        let servers = start(
            0,
            0,
            DemoScenario::FounderInbox,
            "founder",
            "correct-secret",
            false,
            events,
        )
        .expect("start demo servers");
        let accepted = imap_login(servers.imap_address, "founder", "correct-secret");
        let rejected = imap_login(servers.imap_address, "founder", "wrong-secret");
        assert!(accepted.contains(" OK LOGIN completed"), "{accepted}");
        assert!(rejected.contains(" NO authentication failed"), "{rejected}");
        servers.stop();
    }

    fn imap_login(address: std::net::SocketAddr, username: &str, password: &str) -> String {
        let mut stream = TcpStream::connect(address).expect("connect demo IMAP");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("set IMAP test timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("clone IMAP stream"));
        let mut greeting = String::new();
        reader.read_line(&mut greeting).expect("read IMAP greeting");
        write!(stream, "a1 LOGIN \"{username}\" \"{password}\"\r\n").expect("write IMAP login");
        stream.flush().expect("flush IMAP login");
        let mut response = String::new();
        reader.read_line(&mut response).expect("read IMAP login");
        response
    }
}
