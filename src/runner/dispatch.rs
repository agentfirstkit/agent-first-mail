use super::lock::{command_lock, workspace_progress_command};
use super::purge::{ResolvedPurgeAction, resolve_purge_action};
use crate::cli::{
    CaseCommand, CaseDraftAction, CaseNotesAction, CaseSuggestionAction, CaseSuggestionActionKind,
    Command, ConfigAction, ContactCommand, ContactEmailAction, ContactNotesAction,
    ContactPhoneAction, DemoAction, DoctorAction, LogAction, MessageAction,
    MessageAttachmentAction, MessageSuggestionAction, NotificationCommand, NotificationNotesAction,
    NotificationSuggestionAction, NotificationSuggestionActionKind, PushAction, RemoteAction,
    RenderAction, TriageAction, TriageSuggestionAction,
};
use crate::error::{AppError, Result};
use crate::progress::{ProgressCallback, WorkspaceProgressSink};
use crate::store::{DraftChange, NewDraft, ReplyDraft, Workspace};
use crate::types::{
    CaseSuggestedAction, CommandResult, MessageSuggestedAction, NotificationSuggestedAction,
};
use crate::workspace_lock::{LockMode, WorkspaceLock};
use serde_json::{Value, json};
use std::path::Path;

pub fn execute_command(command: Command) -> Result<CommandResult> {
    execute_command_with_progress(command, None, None)
}

pub(super) fn execute_command_with_progress(
    command: Command,
    request_id: Option<&str>,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<CommandResult> {
    let cwd = std::env::current_dir().map_err(|e| AppError::io("current dir", &e))?;
    execute_command_at(command, request_id, &cwd, progress)
}

pub(crate) fn execute_command_at(
    command: Command,
    request_id: Option<&str>,
    cwd: &Path,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<CommandResult> {
    validate_request_id(&command, request_id)?;
    match command {
        Command::Skill { action } => {
            crate::skill_admin::handle_action(action).map(CommandResult::succeeded)
        }
        Command::CloudflareScaffold { path } => {
            crate::sources::cloudflare::scaffold(&cwd.join(path)).map(CommandResult::succeeded)
        }
        Command::CloudflareConsume { reconcile } => {
            crate::sources::cloudflare::consume(cwd, reconcile).map(CommandResult::succeeded)
        }
        Command::Status => execute_status(cwd).map(CommandResult::succeeded),
        Command::Api { .. } => Err(AppError::new(
            "api_dispatch_error",
            "api commands must be handled by the API runner",
        )),
        Command::Demo {
            action: DemoAction::Serve { .. },
        } => Err(AppError::new(
            "demo_dispatch_error",
            "demo serve must be handled by the demo server runner",
        )),
        command => {
            let lock = command_lock(&command, cwd)?;
            // Initialization validates its target before creating the lock directory.
            let _lock = if matches!(
                command,
                Command::Init { .. }
                    | Command::Demo {
                        action: DemoAction::Init { .. }
                    }
            ) {
                None
            } else {
                Some(WorkspaceLock::acquire(&lock.root, lock.mode)?)
            };
            let mut workspace_progress = workspace_progress_command(&command)
                .map(|name| WorkspaceProgressSink::start(&lock.root, name));
            if lock.mode == LockMode::Exclusive
                && !matches!(
                    command,
                    Command::Init { .. }
                        | Command::Demo {
                            action: DemoAction::Init { .. }
                        }
                        | Command::Doctor { .. }
                )
                && let Err(err) = Workspace::at(&lock.root).ensure_no_incomplete_transactions()
            {
                if let Some(sink) = &mut workspace_progress {
                    sink.finish_failure(&err);
                }
                return Err(err);
            }
            let mut progress = progress;
            let should_emit_progress = progress.is_some() || workspace_progress.is_some();
            let result = {
                let mut emit_progress = |phase: &str, fields: Value| {
                    if let Some(callback) = progress.as_deref_mut() {
                        callback(phase, fields.clone());
                    }
                    if let Some(sink) = &mut workspace_progress {
                        sink.update(phase, fields);
                    }
                };
                let progress = if should_emit_progress {
                    Some(&mut emit_progress as &mut ProgressCallback<'_>)
                } else {
                    None
                };
                execute_command_unlocked(command, request_id, cwd, progress)
            };
            if let Some(sink) = &mut workspace_progress {
                match &result {
                    Ok(result) => sink.finish_result(result),
                    Err(err) => sink.finish_failure(err),
                }
            }
            match workspace_progress {
                Some(mut sink) => attach_progress_warnings(result, sink.take_write_warnings()),
                None => result,
            }
        }
    }
}

fn attach_progress_warnings(
    result: Result<CommandResult>,
    warnings: Vec<Value>,
) -> Result<CommandResult> {
    if warnings.is_empty() {
        return result;
    }
    match result {
        Ok(result) => Ok(command_result_with_progress_warnings(result, warnings)),
        Err(err) => Err(error_with_progress_warnings(err, warnings)),
    }
}

fn command_result_with_progress_warnings(
    result: CommandResult,
    warnings: Vec<Value>,
) -> CommandResult {
    let warning_count = warnings.len();
    let mut value = result.value().clone();
    if let Value::Object(map) = &mut value {
        map.insert("progress_warning_count".to_string(), json!(warning_count));
        map.insert("progress_warnings".to_string(), Value::Array(warnings));
    }
    CommandResult::aggregate(value, usize::from(result.is_success()), warning_count)
}

fn error_with_progress_warnings(mut err: AppError, warnings: Vec<Value>) -> AppError {
    let warning_count = warnings.len();
    let mut details = match err.details.take() {
        Some(Value::Object(map)) => map,
        Some(value) => {
            let mut map = serde_json::Map::new();
            map.insert("details".to_string(), value);
            map
        }
        None => serde_json::Map::new(),
    };
    details.insert("progress_warning_count".to_string(), json!(warning_count));
    details.insert("progress_warnings".to_string(), Value::Array(warnings));
    err.details = Some(Value::Object(details));
    err
}

fn execute_command_unlocked(
    command: Command,
    request_id: Option<&str>,
    cwd: &Path,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<CommandResult> {
    let mut counts = (1usize, 0usize);
    let value = match command {
        Command::Api { .. } => Err(AppError::new(
            "api_dispatch_error",
            "api commands must be handled by the API runner",
        )),
        Command::Demo {
            action: DemoAction::Serve { .. },
        } => Err(AppError::new(
            "demo_dispatch_error",
            "demo serve must be handled by the demo server runner",
        )),
        Command::Demo {
            action:
                DemoAction::Init {
                    path,
                    scenario,
                    imap_port,
                    smtp_port,
                },
        } => crate::demo::init(cwd, &path, scenario, imap_port, smtp_port),
        Command::CloudflareScaffold { path } => {
            crate::sources::cloudflare::scaffold(&cwd.join(path))
        }
        Command::CloudflareConsume { .. } | Command::CloudflareWatch => Err(AppError::new(
            "consumer_dispatch_error",
            "Cloudflare consumer must manage its own workspace lock",
        )),
        Command::Init { path } => Workspace::init_command(cwd, path.as_deref()),
        Command::Pull {
            cloudflare,
            files,
            ids,
            dry_run,
            since_days,
            max_new_messages,
            max_total_bytes,
            all_history,
        } => Workspace::discover(cwd)?.pull_sources_with_progress(
            &ids,
            cloudflare,
            files,
            crate::sources::imap::PullOptions::new(
                since_days,
                max_new_messages,
                max_total_bytes,
                all_history,
                dry_run,
            ),
            progress,
        ),
        Command::Config { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                ConfigAction::Show => ws.config_show(),
                ConfigAction::Get { key } => ws.config_get(&key),
                ConfigAction::Set { key, values } => ws.config_set(&key, &values),
                ConfigAction::Add { key, slug, fields } => ws.config_add(&key, &slug, &fields),
                ConfigAction::Remove { key, slug } => ws.config_remove(&key, &slug),
            }
        }
        Command::Remote { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                RemoteAction::Test => ws.remote_test(),
                RemoteAction::Folders => ws.remote_folders(),
            }
        }
        Command::Push {
            dry_run: _,
            confirm,
            action,
        } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                Some(PushAction::List) => ws.push_list(),
                Some(PushAction::Recover {
                    push_id,
                    confirm,
                    settle,
                }) => ws.push_recover(&push_id, confirm.as_deref(), settle),
                // `--dry-run` and `--confirm` are separate registered shapes, so
                // a plan id here always means "apply exactly this plan".
                None => take_command_result(
                    ws.push_with_progress(confirm.as_deref(), progress),
                    &mut counts,
                ),
            }
        }
        Command::Status => execute_status(cwd),
        Command::Doctor { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                None => take_command_result(ws.doctor(), &mut counts),
                Some(DoctorAction::Repair) => take_command_result(ws.doctor_repair(), &mut counts),
            }
        }
        Command::Purge {
            action,
            older_than_days,
            confirm,
        } => {
            let ws = Workspace::discover(cwd)?;
            match resolve_purge_action(action, older_than_days, confirm) {
                ResolvedPurgeAction::Spam {
                    older_than_days,
                    confirm,
                } => ws.purge_spam(older_than_days, confirm.as_deref()),
                ResolvedPurgeAction::Trash {
                    older_than_days,
                    confirm,
                } => ws.purge_trash(older_than_days, confirm.as_deref()),
                ResolvedPurgeAction::Deleted {
                    older_than_days,
                    confirm,
                } => ws.purge_deleted(older_than_days, confirm.as_deref()),
                ResolvedPurgeAction::Discards {
                    older_than_days,
                    confirm,
                } => ws.purge_discards(older_than_days, confirm.as_deref()),
            }
        }
        Command::Skill { action } => crate::skill_admin::handle_action(action),
        Command::Triage { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                TriageAction::List { limit, details } => ws.triage_list(limit, details),
                TriageAction::Suggestion { action } => match action {
                    TriageSuggestionAction::Set(args) => {
                        let suggested_action = message_suggested_action(args.action);
                        idempotent_value(
                            &ws,
                            request_id,
                            "triage_suggestion_set",
                            json!({
                                "message_id": args.message_id,
                                "message_review_base_hash": args.message_review_base_hash,
                                "suggested_action": &suggested_action,
                                "reason": args.reason,
                                "confidence_percent": args.confidence_percent,
                            }),
                            || {
                                ws.set_message_agent_suggestion(
                                    &args.message_id,
                                    &args.message_review_base_hash,
                                    suggested_action,
                                    &args.reason,
                                    args.confidence_percent,
                                )
                            },
                        )
                    }
                    TriageSuggestionAction::Clear {
                        message_id,
                        agent_suggestion_uid,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "triage_suggestion_clear",
                        json!({
                            "message_id": message_id,
                            "agent_suggestion_uid": agent_suggestion_uid,
                        }),
                        || ws.clear_message_suggestion(&message_id, &agent_suggestion_uid),
                    ),
                },
                TriageAction::Review { .. } => Err(AppError::new(
                    "review_dispatch_error",
                    "review commands must be handled by the review session runner",
                )),
            }
        }
        Command::Message { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                MessageAction::Show { message_id } => ws.message_show(&message_id),
                MessageAction::List {
                    sender,
                    subject_contains,
                    since,
                    scope,
                } => ws.message_list(
                    sender.as_deref(),
                    subject_contains.as_deref(),
                    since.as_deref(),
                    scope.as_str(),
                ),
                MessageAction::Spam { message_id, reason } => {
                    if request_id.is_some() {
                        let Some(single_message_id) = message_id.first() else {
                            return Err(AppError::new(
                                "invalid_request",
                                "message disposition requires at least one message id",
                            ));
                        };
                        idempotent_value(
                            &ws,
                            request_id,
                            "message_spam",
                            json!({"message_id": single_message_id, "reason": reason}),
                            || ws.spam_message(single_message_id, reason.as_deref()),
                        )
                    } else {
                        take_command_result(
                            batch_message_disposition(
                                "message_spam_batch",
                                &message_id,
                                reason.as_deref(),
                                |id| ws.spam_message(id, reason.as_deref()),
                            ),
                            &mut counts,
                        )
                    }
                }
                MessageAction::Trash { message_id, reason } => {
                    if request_id.is_some() {
                        let Some(single_message_id) = message_id.first() else {
                            return Err(AppError::new(
                                "invalid_request",
                                "message disposition requires at least one message id",
                            ));
                        };
                        idempotent_value(
                            &ws,
                            request_id,
                            "message_trash",
                            json!({"message_id": single_message_id, "reason": reason}),
                            || ws.trash_message(single_message_id, reason.as_deref()),
                        )
                    } else {
                        take_command_result(
                            batch_message_disposition(
                                "message_trash_batch",
                                &message_id,
                                reason.as_deref(),
                                |id| ws.trash_message(id, reason.as_deref()),
                            ),
                            &mut counts,
                        )
                    }
                }
                MessageAction::Unspam { message_id, reason } => {
                    if request_id.is_some() {
                        let Some(single_message_id) = message_id.first() else {
                            return Err(AppError::new(
                                "invalid_request",
                                "message disposition requires at least one message id",
                            ));
                        };
                        idempotent_value(
                            &ws,
                            request_id,
                            "message_unspam",
                            json!({"message_id": single_message_id, "reason": reason}),
                            || ws.unspam_message(single_message_id, reason.as_deref()),
                        )
                    } else {
                        take_command_result(
                            batch_message_disposition(
                                "message_unspam_batch",
                                &message_id,
                                reason.as_deref(),
                                |id| ws.unspam_message(id, reason.as_deref()),
                            ),
                            &mut counts,
                        )
                    }
                }
                MessageAction::Untrash { message_id, reason } => {
                    if request_id.is_some() {
                        let Some(single_message_id) = message_id.first() else {
                            return Err(AppError::new(
                                "invalid_request",
                                "message disposition requires at least one message id",
                            ));
                        };
                        idempotent_value(
                            &ws,
                            request_id,
                            "message_untrash",
                            json!({"message_id": single_message_id, "reason": reason}),
                            || ws.untrash_message(single_message_id, reason.as_deref()),
                        )
                    } else {
                        take_command_result(
                            batch_message_disposition(
                                "message_untrash_batch",
                                &message_id,
                                reason.as_deref(),
                                |id| ws.untrash_message(id, reason.as_deref()),
                            ),
                            &mut counts,
                        )
                    }
                }
                MessageAction::Attachment { action } => match action {
                    MessageAttachmentAction::Fetch {
                        message_id,
                        part_id,
                    } => ws.fetch_message_attachment(&message_id, part_id.as_deref()),
                },
            }
        }
        Command::Case { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                CaseCommand::Create(args) => idempotent_value_with_precheck(
                    &ws,
                    request_id,
                    "case_create",
                    json!({
                        "name": args.name,
                        "group": args.group,
                        "message": args.message,
                        "summary": args.summary,
                        "reason": args.reason,
                    }),
                    || {
                        ws.prepare_case_create(
                            &args.name,
                            args.group.as_deref(),
                            args.message.as_deref(),
                            args.summary.as_deref(),
                            args.reason.as_deref(),
                        )
                    },
                    |prepared| ws.create_prepared_case(prepared),
                ),
                CaseCommand::List { archived, all } => ws.case_list(archived, all),
                CaseCommand::Show { case_ref } => ws.case_show(&case_ref),
                CaseCommand::Review { .. } => Err(AppError::new(
                    "review_dispatch_error",
                    "review commands must be handled by the review session runner",
                )),
                CaseCommand::Suggestion { action } => match action {
                    CaseSuggestionAction::Set(args) => {
                        let suggested_action = case_suggested_action(args.action);
                        idempotent_value(
                            &ws,
                            request_id,
                            "case_suggestion_set",
                            json!({
                                "case_ref": args.case_ref,
                                "case_review_base_hash": args.case_review_base_hash,
                                "suggested_action": &suggested_action,
                                "reason": args.reason,
                                "confidence_percent": args.confidence_percent,
                            }),
                            || {
                                ws.set_case_agent_suggestion(
                                    &args.case_ref,
                                    &args.case_review_base_hash,
                                    suggested_action,
                                    &args.reason,
                                    args.confidence_percent,
                                )
                            },
                        )
                    }
                    CaseSuggestionAction::Clear {
                        case_ref,
                        agent_suggestion_uid,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "case_suggestion_clear",
                        json!({
                            "case_ref": case_ref,
                            "agent_suggestion_uid": agent_suggestion_uid,
                        }),
                        || ws.clear_case_suggestion(&case_ref, &agent_suggestion_uid),
                    ),
                },
                CaseCommand::Add {
                    case_ref,
                    message_id,
                    summary,
                    reason,
                } => {
                    if request_id.is_some() {
                        let Some(single_message_id) = message_id.first() else {
                            return Err(AppError::new(
                                "invalid_request",
                                "message disposition requires at least one message id",
                            ));
                        };
                        idempotent_value(
                            &ws,
                            request_id,
                            "case_add",
                            json!({
                                "case_ref": case_ref,
                                "message_id": single_message_id,
                                "summary": summary,
                                "reason": reason,
                            }),
                            || {
                                ws.add_message_to_case(
                                    &case_ref,
                                    single_message_id,
                                    summary.as_deref(),
                                    reason.as_deref(),
                                )
                            },
                        )
                    } else {
                        take_command_result(
                            batch_message_disposition(
                                "case_message_added_batch",
                                &message_id,
                                reason.as_deref(),
                                |id| {
                                    ws.add_message_to_case(
                                        &case_ref,
                                        id,
                                        summary.as_deref(),
                                        reason.as_deref(),
                                    )
                                },
                            ),
                            &mut counts,
                        )
                    }
                }
                CaseCommand::SetSummary {
                    case_ref,
                    message_id,
                    summary,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "case_set_summary",
                    json!({
                        "case_ref": case_ref,
                        "message_id": message_id,
                        "summary": summary,
                        "reason": reason,
                    }),
                    || ws.case_set_summary(&case_ref, &message_id, &summary, reason.as_deref()),
                ),
                CaseCommand::Remove {
                    case_ref,
                    message_id,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "case_remove",
                    json!({"case_ref": case_ref, "message_id": message_id, "reason": reason}),
                    || ws.remove_message_from_case(&case_ref, &message_id, reason.as_deref()),
                ),
                CaseCommand::Move {
                    case_ref,
                    group,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "case_move",
                    json!({"case_ref": case_ref, "group": group, "reason": reason}),
                    || ws.move_case(&case_ref, &group, reason.as_deref()),
                ),
                CaseCommand::Rename {
                    case_ref,
                    name,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "case_rename",
                    json!({"case_ref": case_ref, "name": name, "reason": reason}),
                    || ws.rename_case(&case_ref, &name, reason.as_deref()),
                ),
                CaseCommand::Notes { action } => match action {
                    CaseNotesAction::Show { case_ref } => ws.case_notes_show(&case_ref),
                    CaseNotesAction::Append { case_ref, text } => idempotent_value(
                        &ws,
                        request_id,
                        "case_notes_append",
                        json!({"case_ref": case_ref, "text": text}),
                        || ws.case_notes_append(&case_ref, &text),
                    ),
                    CaseNotesAction::Replace { case_ref, text } => idempotent_value(
                        &ws,
                        request_id,
                        "case_notes_replace",
                        json!({"case_ref": case_ref, "text": text}),
                        || ws.case_notes_replace(&case_ref, &text),
                    ),
                },
                CaseCommand::Archive { case_ref, reason } => idempotent_value(
                    &ws,
                    request_id,
                    "case_archive",
                    json!({"case_ref": case_ref, "reason": reason}),
                    || ws.archive_case(&case_ref, reason.as_deref()),
                ),
                CaseCommand::Unarchive {
                    case_ref,
                    group,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "case_unarchive",
                    json!({"case_ref": case_ref, "group": group, "reason": reason}),
                    || ws.unarchive_case(&case_ref, &group, reason.as_deref()),
                ),
                CaseCommand::Trash { case_ref, reason } => idempotent_value(
                    &ws,
                    request_id,
                    "case_trash",
                    json!({"case_ref": case_ref, "reason": reason}),
                    || ws.trash_case(&case_ref, reason.as_deref()),
                ),
                CaseCommand::Delete {
                    case_ref,
                    allow_notes,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "case_delete",
                    json!({"case_ref": case_ref, "allow_notes": allow_notes, "reason": reason}),
                    || ws.delete_case(&case_ref, allow_notes, reason.as_deref()),
                ),
                CaseCommand::Tag {
                    case_ref,
                    tag,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "case_tag",
                    json!({"case_ref": case_ref, "tag": tag, "reason": reason}),
                    || ws.tag_case(&case_ref, &tag, reason.as_deref()),
                ),
                CaseCommand::Untag {
                    case_ref,
                    tag,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "case_untag",
                    json!({"case_ref": case_ref, "tag": tag, "reason": reason}),
                    || ws.untag_case(&case_ref, &tag, reason.as_deref()),
                ),
                CaseCommand::Draft { action } => match action {
                    CaseDraftAction::New {
                        case_ref,
                        to,
                        cc,
                        bcc,
                        subject,
                        identity,
                        body,
                        body_file,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "case_draft_new",
                        json!({
                            "case_ref": case_ref,
                            "to": to,
                            "cc": cc,
                            "bcc": bcc,
                            "subject": subject,
                            "identity": identity,
                            "body": body,
                            "body_file": body_file,
                        }),
                        || {
                            ws.create_draft(
                                &case_ref,
                                NewDraft {
                                    to: &to,
                                    cc: &cc,
                                    bcc: &bcc,
                                    subject: &subject,
                                    identity: identity.as_deref(),
                                    body: body.as_deref(),
                                    body_file: body_file.as_deref(),
                                },
                            )
                        },
                    ),
                    CaseDraftAction::Reply {
                        case_ref,
                        message_id,
                        bcc,
                        identity,
                        body,
                        body_file,
                        all,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "case_draft_reply",
                        json!({
                            "case_ref": case_ref,
                            "message_id": message_id,
                            "bcc": bcc,
                            "all": all,
                            "identity": identity,
                            "body": body,
                            "body_file": body_file,
                        }),
                        || {
                            ws.reply_to_message_with_options(
                                &case_ref,
                                ReplyDraft {
                                    message_id: &message_id,
                                    reply_all: all,
                                    bcc: &bcc,
                                    identity: identity.as_deref(),
                                    body: body.as_deref(),
                                    body_file: body_file.as_deref(),
                                },
                            )
                        },
                    ),
                    CaseDraftAction::Change {
                        case_ref,
                        draft_name,
                        subject,
                        to,
                        cc,
                        clear_cc,
                        bcc,
                        clear_bcc,
                        identity,
                        body,
                        body_file,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "case_draft_change",
                        json!({
                            "case_ref": case_ref,
                            "draft_name": draft_name,
                            "subject": subject,
                            "to": to,
                            "cc": cc,
                            "clear_cc": clear_cc,
                            "bcc": bcc,
                            "clear_bcc": clear_bcc,
                            "identity": identity,
                            "body": body,
                            "body_file": body_file,
                        }),
                        || {
                            ws.change_draft(
                                &case_ref,
                                &draft_name,
                                DraftChange {
                                    subject: subject.as_deref(),
                                    to: &to,
                                    cc: &cc,
                                    clear_cc,
                                    bcc: &bcc,
                                    clear_bcc,
                                    identity: identity.as_deref(),
                                    body: body.as_deref(),
                                    body_file: body_file.as_deref(),
                                },
                            )
                        },
                    ),
                    CaseDraftAction::Show {
                        case_ref,
                        draft_name,
                    } => ws.show_draft(&case_ref, &draft_name),
                    CaseDraftAction::Review { .. } => Err(AppError::new(
                        "review_dispatch_error",
                        "review commands must be handled by the review session runner",
                    )),
                    CaseDraftAction::List { case_ref } => ws.list_drafts(&case_ref),
                    CaseDraftAction::Validate {
                        case_ref,
                        draft_name,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "case_draft_validate",
                        json!({"case_ref": case_ref, "draft_name": draft_name}),
                        || ws.validate_draft(&case_ref, &draft_name),
                    ),
                    CaseDraftAction::Upload {
                        case_ref,
                        draft_name,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "case_draft_upload",
                        json!({"case_ref": case_ref, "draft_name": draft_name}),
                        || ws.upload_draft(&case_ref, &draft_name),
                    ),
                    CaseDraftAction::Send {
                        case_ref,
                        draft_name,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "case_draft_send",
                        json!({"case_ref": case_ref, "draft_name": draft_name}),
                        || ws.send_draft(&case_ref, &draft_name),
                    ),
                    CaseDraftAction::Attach {
                        case_ref,
                        draft_name,
                        path,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "case_draft_attach",
                        json!({"case_ref": case_ref, "draft_name": draft_name, "path": path}),
                        || ws.attach_file_to_draft(&case_ref, &draft_name, &path),
                    ),
                    CaseDraftAction::Remove {
                        case_ref,
                        draft_name,
                        reason,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "case_draft_remove",
                        json!({"case_ref": case_ref, "draft_name": draft_name, "reason": reason}),
                        || ws.remove_draft(&case_ref, &draft_name, reason.as_deref()),
                    ),
                },
                CaseCommand::Merge {
                    case_ref,
                    other_case_ref,
                    reason,
                } => ws.merge_case(&case_ref, &other_case_ref, reason.as_deref()),
            }
        }
        Command::Contact { action } => {
            use crate::store::contacts::{ContactExtractSource, NewContact};
            let ws = Workspace::discover(cwd)?;
            match action {
                ContactCommand::Create(args) => idempotent_value(
                    &ws,
                    request_id,
                    "contact_create",
                    json!({
                        "name": args.name,
                        "group": args.group,
                        "emails": args.emails,
                        "phones": args.phones,
                        "org": args.org,
                        "role": args.role,
                        "tags": args.tags,
                        "reason": args.reason,
                    }),
                    || {
                        ws.contact_create(NewContact {
                            name: &args.name,
                            group: args.group.as_deref(),
                            emails: &args.emails,
                            phones: &args.phones,
                            org: args.org.as_deref(),
                            role: args.role.as_deref(),
                            tags: &args.tags,
                            reason: args.reason.as_deref(),
                        })
                    },
                ),
                ContactCommand::List {
                    group,
                    tag,
                    org,
                    role,
                } => ws.contact_list(
                    group.as_deref(),
                    tag.as_deref(),
                    org.as_deref(),
                    role.as_deref(),
                ),
                ContactCommand::Show { contact_ref } => ws.contact_show(&contact_ref),
                ContactCommand::Move {
                    contact_ref,
                    group,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "contact_move",
                    json!({"contact_ref": contact_ref, "group": group, "reason": reason}),
                    || ws.contact_move(&contact_ref, &group, reason.as_deref()),
                ),
                ContactCommand::Rename {
                    contact_ref,
                    name,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "contact_rename",
                    json!({"contact_ref": contact_ref, "name": name, "reason": reason}),
                    || ws.contact_rename(&contact_ref, &name, reason.as_deref()),
                ),
                ContactCommand::Email { action } => match action {
                    ContactEmailAction::Add {
                        contact_ref,
                        email,
                        reason,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "contact_email_add",
                        json!({"contact_ref": contact_ref, "email": email, "reason": reason}),
                        || ws.contact_email_add(&contact_ref, &email, reason.as_deref()),
                    ),
                    ContactEmailAction::Remove {
                        contact_ref,
                        email,
                        reason,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "contact_email_remove",
                        json!({"contact_ref": contact_ref, "email": email, "reason": reason}),
                        || ws.contact_email_remove(&contact_ref, &email, reason.as_deref()),
                    ),
                },
                ContactCommand::Phone { action } => match action {
                    ContactPhoneAction::Add {
                        contact_ref,
                        phone,
                        reason,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "contact_phone_add",
                        json!({"contact_ref": contact_ref, "phone": phone, "reason": reason}),
                        || ws.contact_phone_add(&contact_ref, &phone, reason.as_deref()),
                    ),
                    ContactPhoneAction::Remove {
                        contact_ref,
                        phone,
                        reason,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "contact_phone_remove",
                        json!({"contact_ref": contact_ref, "phone": phone, "reason": reason}),
                        || ws.contact_phone_remove(&contact_ref, &phone, reason.as_deref()),
                    ),
                },
                ContactCommand::Tag {
                    contact_ref,
                    tag,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "contact_tag",
                    json!({"contact_ref": contact_ref, "tag": tag, "reason": reason}),
                    || ws.contact_tag(&contact_ref, &tag, reason.as_deref()),
                ),
                ContactCommand::Untag {
                    contact_ref,
                    tag,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "contact_untag",
                    json!({"contact_ref": contact_ref, "tag": tag, "reason": reason}),
                    || ws.contact_untag(&contact_ref, &tag, reason.as_deref()),
                ),
                ContactCommand::Notes { action } => match action {
                    ContactNotesAction::Show { contact_ref } => ws.contact_notes_show(&contact_ref),
                    ContactNotesAction::Append { contact_ref, text } => idempotent_value(
                        &ws,
                        request_id,
                        "contact_notes_append",
                        json!({"contact_ref": contact_ref, "text": text}),
                        || ws.contact_notes_append(&contact_ref, &text),
                    ),
                    ContactNotesAction::Replace { contact_ref, text } => idempotent_value(
                        &ws,
                        request_id,
                        "contact_notes_replace",
                        json!({"contact_ref": contact_ref, "text": text}),
                        || ws.contact_notes_replace(&contact_ref, &text),
                    ),
                },
                ContactCommand::Delete {
                    contact_ref,
                    allow_notes,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "contact_delete",
                    json!({"contact_ref": contact_ref, "allow_notes": allow_notes, "reason": reason}),
                    || ws.contact_delete(&contact_ref, allow_notes, reason.as_deref()),
                ),
                ContactCommand::Extract {
                    from_triage,
                    from_case,
                    all,
                    group,
                } => {
                    let source = if from_triage {
                        ContactExtractSource::FromTriage
                    } else if let Some(case_ref) = from_case {
                        ContactExtractSource::FromCase(case_ref)
                    } else if all {
                        ContactExtractSource::All
                    } else {
                        ContactExtractSource::FromTriage
                    };
                    ws.contact_extract(source, group.as_deref())
                }
            }
        }
        Command::Notification { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                NotificationCommand::Create(args) => idempotent_value(
                    &ws,
                    request_id,
                    "notification_create",
                    json!({
                        "name": args.name,
                        "message": args.message,
                        "summary": args.summary,
                        "reason": args.reason,
                    }),
                    || {
                        ws.create_notification(
                            &args.name,
                            args.message.as_deref(),
                            args.summary.as_deref(),
                            args.reason.as_deref(),
                        )
                    },
                ),
                NotificationCommand::Add {
                    notification_ref,
                    message_id,
                    summary,
                    reason,
                } => {
                    if request_id.is_some() {
                        let Some(single_message_id) = message_id.first() else {
                            return Err(AppError::new(
                                "invalid_request",
                                "message disposition requires at least one message id",
                            ));
                        };
                        idempotent_value(
                            &ws,
                            request_id,
                            "notification_add",
                            json!({
                                "notification_ref": notification_ref,
                                "message_id": single_message_id,
                                "summary": summary,
                                "reason": reason,
                            }),
                            || {
                                ws.notification_add_message(
                                    single_message_id,
                                    &notification_ref,
                                    Some(summary.as_str()),
                                    reason.as_deref(),
                                )
                            },
                        )
                    } else {
                        take_command_result(
                            batch_message_disposition(
                                "notification_message_added_batch",
                                &message_id,
                                reason.as_deref(),
                                |id| {
                                    ws.notification_add_message(
                                        id,
                                        &notification_ref,
                                        Some(summary.as_str()),
                                        reason.as_deref(),
                                    )
                                },
                            ),
                            &mut counts,
                        )
                    }
                }
                NotificationCommand::List => ws.notification_list(),
                NotificationCommand::Show { notification_ref } => {
                    ws.notification_show(&notification_ref)
                }
                NotificationCommand::Review { .. } => Err(AppError::new(
                    "review_dispatch_error",
                    "review commands must be handled by the review session runner",
                )),
                NotificationCommand::Suggestion { action } => match action {
                    NotificationSuggestionAction::Set(args) => {
                        let suggested_action = notification_suggested_action(args.action);
                        idempotent_value(
                            &ws,
                            request_id,
                            "notification_suggestion_set",
                            json!({
                                "notification_ref": args.notification_ref,
                                "notification_review_base_hash": args.notification_review_base_hash,
                                "suggested_action": &suggested_action,
                                "reason": args.reason,
                                "confidence_percent": args.confidence_percent,
                            }),
                            || {
                                ws.set_notification_agent_suggestion(
                                    &args.notification_ref,
                                    &args.notification_review_base_hash,
                                    suggested_action,
                                    &args.reason,
                                    args.confidence_percent,
                                )
                            },
                        )
                    }
                    NotificationSuggestionAction::Clear {
                        notification_ref,
                        agent_suggestion_uid,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "notification_suggestion_clear",
                        json!({
                            "notification_ref": notification_ref,
                            "agent_suggestion_uid": agent_suggestion_uid,
                        }),
                        || {
                            ws.clear_notification_suggestion(
                                &notification_ref,
                                &agent_suggestion_uid,
                            )
                        },
                    ),
                },
                NotificationCommand::Remove {
                    notification_ref,
                    message_id,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "notification_remove",
                    json!({"notification_ref": notification_ref, "message_id": message_id, "reason": reason}),
                    || ws.notification_remove(&notification_ref, &message_id, reason.as_deref()),
                ),
                NotificationCommand::Move {
                    notification_ref,
                    message_id,
                    new_notification_ref,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "notification_move",
                    json!({
                        "notification_ref": notification_ref,
                        "message_id": message_id,
                        "new_notification_ref": new_notification_ref,
                        "reason": reason,
                    }),
                    || {
                        ws.notification_move(
                            &notification_ref,
                            &message_id,
                            &new_notification_ref,
                            reason.as_deref(),
                        )
                    },
                ),
                NotificationCommand::Rename {
                    notification_ref,
                    name,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "notification_rename",
                    json!({"notification_ref": notification_ref, "name": name, "reason": reason}),
                    || ws.notification_rename(&notification_ref, &name, reason.as_deref()),
                ),
                NotificationCommand::Delete {
                    notification_ref,
                    allow_notes,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "notification_delete",
                    json!({"notification_ref": notification_ref, "allow_notes": allow_notes, "reason": reason}),
                    || ws.notification_delete(&notification_ref, allow_notes, reason.as_deref()),
                ),
                NotificationCommand::SetSummary {
                    notification_ref,
                    message_id,
                    summary,
                    reason,
                } => idempotent_value(
                    &ws,
                    request_id,
                    "notification_set_summary",
                    json!({
                        "notification_ref": notification_ref,
                        "message_id": message_id,
                        "summary": summary,
                        "reason": reason,
                    }),
                    || {
                        ws.notification_set_summary(
                            &notification_ref,
                            &message_id,
                            &summary,
                            reason.as_deref(),
                        )
                    },
                ),
                NotificationCommand::Notes { action } => match action {
                    NotificationNotesAction::Show { notification_ref } => {
                        ws.notification_notes_show(&notification_ref)
                    }
                    NotificationNotesAction::Append {
                        notification_ref,
                        text,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "notification_notes_append",
                        json!({"notification_ref": notification_ref, "text": text}),
                        || ws.notification_notes_append(&notification_ref, &text),
                    ),
                    NotificationNotesAction::Replace {
                        notification_ref,
                        text,
                    } => idempotent_value(
                        &ws,
                        request_id,
                        "notification_notes_replace",
                        json!({"notification_ref": notification_ref, "text": text}),
                        || ws.notification_notes_replace(&notification_ref, &text),
                    ),
                },
            }
        }
        Command::Render { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                RenderAction::Refresh => ws.render_refresh_with_progress(progress),
                RenderAction::Templates { force } => ws.render_templates(force),
            }
        }
        Command::Log { action } => {
            let ws = Workspace::discover(cwd)?;
            let result = match action {
                LogAction::List { limit } => ws.log_list(limit),
                LogAction::Tail => ws.log_tail(),
                LogAction::Message { message_id } => ws.log_message(&message_id),
                LogAction::Case { case_uid } => ws.log_case(&case_uid),
                LogAction::Notification { notification_uid } => {
                    ws.log_notification(&notification_uid)
                }
            };
            take_command_result(command_result_from_warnings(result), &mut counts)
        }
    }?;
    Ok(CommandResult::aggregate(value, counts.0, counts.1))
}

fn message_suggested_action(action: MessageSuggestionAction) -> MessageSuggestedAction {
    match action {
        MessageSuggestionAction::CaseCreate { name, group } => MessageSuggestedAction::CaseCreate {
            case_name: name,
            group,
        },
        MessageSuggestionAction::CaseAdd { case_ref } => {
            MessageSuggestedAction::CaseAdd { case_ref }
        }
        MessageSuggestionAction::NotificationCreate { name, summary } => {
            MessageSuggestedAction::NotificationCreate {
                notification_name: name,
                summary,
            }
        }
        MessageSuggestionAction::NotificationAdd {
            notification_ref,
            summary,
        } => MessageSuggestedAction::NotificationAdd {
            notification_ref,
            summary,
        },
        MessageSuggestionAction::Spam => MessageSuggestedAction::Spam,
        MessageSuggestionAction::Trash => MessageSuggestedAction::Trash,
    }
}

fn case_suggested_action(action: CaseSuggestionActionKind) -> CaseSuggestedAction {
    match action {
        CaseSuggestionActionKind::Archive => CaseSuggestedAction::CaseArchive,
        CaseSuggestionActionKind::Unarchive { group } => {
            CaseSuggestedAction::CaseUnarchive { group }
        }
        CaseSuggestionActionKind::Move { group } => CaseSuggestedAction::CaseMove { group },
        CaseSuggestionActionKind::Rename { name } => {
            CaseSuggestedAction::CaseRename { case_name: name }
        }
        CaseSuggestionActionKind::Tag { tag } => CaseSuggestedAction::CaseTag { tag },
        CaseSuggestionActionKind::Untag { tag } => CaseSuggestedAction::CaseUntag { tag },
        CaseSuggestionActionKind::Trash => CaseSuggestedAction::CaseTrash,
        CaseSuggestionActionKind::Delete { allow_notes } => {
            CaseSuggestedAction::CaseDelete { allow_notes }
        }
    }
}

fn notification_suggested_action(
    action: NotificationSuggestionActionKind,
) -> NotificationSuggestedAction {
    match action {
        NotificationSuggestionActionKind::Rename { name } => {
            NotificationSuggestedAction::NotificationRename {
                notification_name: name,
            }
        }
        NotificationSuggestionActionKind::Delete { allow_notes } => {
            NotificationSuggestedAction::NotificationDelete { allow_notes }
        }
    }
}

fn idempotent_value<F>(
    workspace: &Workspace,
    request_id: Option<&str>,
    command: &str,
    input: Value,
    operation: F,
) -> Result<Value>
where
    F: FnOnce() -> Result<Value>,
{
    match request_id {
        Some(request_id) => workspace.run_idempotent_request(request_id, command, input, operation),
        None => operation(),
    }
}

fn idempotent_value_with_precheck<T, P, F>(
    workspace: &Workspace,
    request_id: Option<&str>,
    command: &str,
    input: Value,
    precheck: P,
    operation: F,
) -> Result<Value>
where
    P: FnOnce() -> Result<T>,
    F: FnOnce(T) -> Result<Value>,
{
    match request_id {
        Some(request_id) => workspace
            .run_idempotent_request_with_precheck(request_id, command, input, precheck, operation),
        None => operation(precheck()?),
    }
}

pub(super) fn validate_request_id(command: &Command, request_id: Option<&str>) -> Result<()> {
    if request_id.is_some() && !request_id_supported(command) {
        return Err(AppError::new(
            "request_id_not_supported",
            "--request-id is only supported for selected non-idempotent local mutations",
        ));
    }
    Ok(())
}

fn request_id_supported(command: &Command) -> bool {
    match command {
        Command::Triage {
            action: TriageAction::Suggestion { .. },
        } => true,
        Command::Message {
            action:
                MessageAction::Spam { message_id, .. }
                | MessageAction::Trash { message_id, .. }
                | MessageAction::Unspam { message_id, .. }
                | MessageAction::Untrash { message_id, .. },
        } => message_id.len() == 1,
        Command::Case { action } => match action {
            CaseCommand::Create(_) => true,
            CaseCommand::Suggestion { .. } => true,
            CaseCommand::Add { message_id, .. } if message_id.len() == 1 => true,
            CaseCommand::Move { .. }
            | CaseCommand::Rename { .. }
            | CaseCommand::SetSummary { .. }
            | CaseCommand::Remove { .. }
            | CaseCommand::Delete { .. }
            | CaseCommand::Archive { .. }
            | CaseCommand::Unarchive { .. }
            | CaseCommand::Trash { .. }
            | CaseCommand::Tag { .. }
            | CaseCommand::Untag { .. } => true,
            CaseCommand::Notes {
                action: CaseNotesAction::Append { .. } | CaseNotesAction::Replace { .. },
            } => true,
            CaseCommand::Draft { action } => {
                matches!(
                    action,
                    CaseDraftAction::New { .. }
                        | CaseDraftAction::Reply { .. }
                        | CaseDraftAction::Change { .. }
                        | CaseDraftAction::Validate { .. }
                        | CaseDraftAction::Upload { .. }
                        | CaseDraftAction::Send { .. }
                        | CaseDraftAction::Attach { .. }
                        | CaseDraftAction::Remove { .. }
                )
            }
            _ => false,
        },
        Command::Contact { action } => matches!(
            action,
            ContactCommand::Create(_)
                | ContactCommand::Move { .. }
                | ContactCommand::Rename { .. }
                | ContactCommand::Email { .. }
                | ContactCommand::Phone { .. }
                | ContactCommand::Tag { .. }
                | ContactCommand::Untag { .. }
                | ContactCommand::Delete { .. }
                | ContactCommand::Notes {
                    action: ContactNotesAction::Append { .. } | ContactNotesAction::Replace { .. },
                }
        ),
        Command::Notification { action } => match action {
            NotificationCommand::Create(_) => true,
            NotificationCommand::Suggestion { .. } => true,
            NotificationCommand::Add { message_id, .. } => message_id.len() == 1,
            NotificationCommand::Remove { .. }
            | NotificationCommand::Move { .. }
            | NotificationCommand::Rename { .. }
            | NotificationCommand::SetSummary { .. }
            | NotificationCommand::Delete { .. } => true,
            NotificationCommand::Notes {
                action:
                    NotificationNotesAction::Append { .. } | NotificationNotesAction::Replace { .. },
            } => true,
            _ => false,
        },
        _ => false,
    }
}

fn execute_status(cwd: &Path) -> Result<Value> {
    let ws = Workspace::discover(cwd)?;
    let mut progress = crate::progress::workspace_status_progress(ws.root())?;
    let Some(_lock) = WorkspaceLock::try_acquire(ws.root(), LockMode::Shared)? else {
        return Ok(json!({
            "code": "status",
            "workspace_locked": true,
            "progress": progress,
            "hint": "Workspace counts are omitted while another afmail command is using this workspace; retry after it finishes for full counts."
        }));
    };
    mark_unlocked_running_progress_stale(&mut progress);
    let mut status = ws.status()?;
    if let Value::Object(map) = &mut status {
        map.insert("progress".to_string(), progress);
    }
    Ok(status)
}

fn mark_unlocked_running_progress_stale(progress: &mut Value) {
    let Value::Object(map) = progress else {
        return;
    };
    if map.get("status").and_then(Value::as_str) != Some("running") {
        return;
    }
    map.insert("status".to_string(), json!("stale"));
}

fn batch_message_disposition<F>(
    _code: &'static str,
    message_ids: &[String],
    reason: Option<&str>,
    mut op: F,
) -> Result<CommandResult>
where
    F: FnMut(&str) -> Result<Value>,
{
    if message_ids.is_empty() {
        return Err(AppError::new(
            "invalid_request",
            "message disposition requires at least one message id",
        ));
    }
    let mut results = Vec::with_capacity(message_ids.len());
    let mut succeeded_count = 0usize;
    let mut failed_count = 0usize;
    for message_id in message_ids {
        match op(message_id) {
            Ok(value) => {
                succeeded_count += 1;
                results.push(value);
            }
            Err(err) => {
                failed_count += 1;
                let mut value = err.to_error_value();
                if let Value::Object(map) = &mut value {
                    map.insert("message_id".to_string(), json!(message_id));
                }
                results.push(value);
            }
        }
    }
    Ok(CommandResult::aggregate(
        json!({
            "message_ids": message_ids,
            "reason": reason,
            "count": message_ids.len(),
            "succeeded_count": succeeded_count,
            "failed_count": failed_count,
            "results": results,
        }),
        succeeded_count,
        failed_count,
    ))
}

fn take_command_result(
    result: Result<CommandResult>,
    counts: &mut (usize, usize),
) -> Result<Value> {
    result.map(|result| {
        *counts = if result.is_success() {
            (1, 0)
        } else if result.is_partial() {
            (1, 1)
        } else {
            (0, 1)
        };
        result.into_value()
    })
}

fn command_result_from_warnings(result: Result<Value>) -> Result<CommandResult> {
    result.map(|value| {
        let warning_count = value
            .get("warning_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        CommandResult::aggregate(value, 1, warning_count as usize)
    })
}
