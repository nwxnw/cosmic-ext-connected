//! `SmsConversationStore` — owns SMS conversation state, message caches,
//! subscription orchestration, optimistic-send state, contacts, and SMS
//! notification dedup.

use crate::app::{LoadingPhase, Message, SmsLoadingState};
use crate::config::Config;
use crate::constants::notifications::NORMAL_NOTIFICATION_TIMEOUT_MS;
use crate::constants::sms::{MESSAGES_PER_PAGE, MMS_MAX_BYTES, NEW_MESSAGE_SYNC_INDICATOR_SECS};
use crate::fl;
use crate::sms::logical::{merge_into_logical, split_candidate_thread_ids, LogicalConversation};
use crate::sms::{
    conversation_list_subscription, request_attachment_async, request_older_messages_async,
    send_new_sms_async, send_sms_async, view_conversation_list, view_message_thread,
    view_new_message, ConversationListParams, MessageThreadParams, NewMessageParams,
};
use crate::subscriptions::conversation_message_subscription;
use cosmic::iced::widget::scrollable;
use cosmic::iced::{clipboard, Subscription};
use cosmic::widget;
use cosmic::Element;
use kdeconnect_dbus::contacts::ContactLookup;
use kdeconnect_dbus::plugins::{
    is_address_valid, ConversationSummary, MessageType, SmsMessage, OPTIMISTIC_MESSAGE_UID,
};
use kdeconnect_dbus::{dedupe_addresses, normalize_phone_number, phone_suffix};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::Connection;

/// Read-only context the parent app passes to the store on each call.
///
/// `conn` is `Option` because the app may not yet have a D-Bus connection
/// when an SMS message arrives; arms that need it guard internally.
pub struct SmsCtx<'a> {
    pub conn: Option<&'a Arc<Mutex<Connection>>>,
    pub config: &'a Config,
}

/// A file staged for the next send, validated at pick time.
#[derive(Debug, Clone)]
pub struct StagedAttachment {
    pub path: std::path::PathBuf,
    /// File size at pick time, in bytes
    pub size: u64,
}

impl StagedAttachment {
    /// This file plus `text` exceeds `MMS_MAX_BYTES`: Send is refused and the chip says why
    pub fn too_large(&self, text: &str) -> bool {
        self.size + text.len() as u64 > MMS_MAX_BYTES
    }
}

/// Reply from the store back to the parent app describing app-level
/// state changes the caller must apply.
#[derive(Debug)]
pub enum SmsReply {
    /// Emit a transient status message (3s auto-clear).
    Status(String),
    /// Set or clear `status_message` directly without auto-clear.
    /// Used for sticky "Loading…" indicators that pair with explicit clear.
    SetStatus(Option<String>),
    /// New-message send succeeded: set status_message + return to ConversationList.
    NewMessageSent(String),
    /// No app-level state change required.
    NoOp,
}

/// Which SMS sub-view the parent app is rendering.
#[derive(Debug, Clone, Copy)]
pub enum SmsViewMode {
    ConversationList,
    MessageThread,
    NewMessage,
}

/// In-flight older-page load. `Some` from firing the range request until the
/// page settles; `None` otherwise. Completion is detected on the streaming path
/// (see the two detectors below).
pub(crate) struct OlderPageLoad {
    /// Per-thread pre-dedup signal count: how far through the requested range the
    /// daemon has got (mirrors its numHandled). Position, not progress.
    served: HashMap<i64, u32>,
    /// Per-thread messages actually inserted. Progress: a full page of these settles.
    new: HashMap<i64, u32>,
    /// Per-thread range end at fire time (`loaded_t + page_size`), as the request carries
    requested: HashMap<i64, u32>,
    /// Per-thread: a `conversationLoaded` arrived after the cache burst, and whether
    /// it grew the daemon's known count past the last one seen.
    grew: HashMap<i64, bool>,
    /// Merged threads whose page hasn't settled yet; finalize when empty
    pending: HashSet<i64>,
    /// Requested page size (== MESSAGES_PER_PAGE).
    page_size: u32,
    /// `messages.len()` captured before the request, for the settle log's prepended count.
    len_before: usize,
    /// Which detector settled each thread. Capture-log only.
    settled_by: HashMap<i64, &'static str>,
}

/// Whether one thread of an older page has settled, and if so its `has_more`.
/// `new >= page`: a full page of history. `grew` is `Some` once a
/// `conversationLoaded` arrives after the cache burst, the last signal the daemon
/// sends for the page: more exists if the cache filled the request or the fetch
/// added uids.
fn page_settle(
    page: u32,
    requested: u32,
    served: u32,
    new: u32,
    grew: Option<bool>,
) -> Option<(bool, &'static str)> {
    if new >= page {
        return Some((true, "full-page"));
    }
    let grew = grew?;
    Some((served >= requested || grew, "conversation-loaded"))
}

/// The display floor of a merged set: the newest oldest-loaded date among
/// threads that still have more (unknown counts as more). Messages older than
/// it sit above history the daemon has not served yet, so the view hides them
/// and an older page targets only the threads at the floor. `None` when no
/// thread has more.
fn display_floor(
    messages: &[SmsMessage],
    threads: &[i64],
    has_more: &HashMap<i64, bool>,
) -> Option<i64> {
    threads
        .iter()
        .filter(|t| *has_more.get(t).unwrap_or(&true))
        .filter_map(|&t| {
            messages
                .iter()
                .filter(|m| m.thread_id == t)
                .map(|m| m.date)
                .min()
        })
        .max()
}

pub struct SmsConversationStore {
    // Active SMS device
    pub(crate) sms_device_id: Option<String>,
    pub(crate) sms_device_name: Option<String>,

    // Conversation list
    /// Raw per-thread conversations from the daemon. Source of truth for the
    /// derived `conversations` list — the merge toggle and any reaction-bucket
    /// re-derivation works off this cache without re-fetching.
    pub(crate) raw_conversations: Vec<ConversationSummary>,
    pub(crate) conversations: Vec<LogicalConversation>,
    pub(crate) sms_prefetch: Option<(String, Vec<ConversationSummary>)>,
    pub(crate) conversation_sync_active: bool,
    pub(crate) new_message_sync_active: bool,
    pub(crate) conversation_list_subscription_active: bool,
    pub(crate) message_sync_active: bool,
    pub(crate) conversation_load_active: bool,
    pub(crate) conversation_list_subscription_generation: u32,
    pub(crate) conversation_message_subscription_generation: u32,
    pub(crate) initial_load_complete: bool,

    // Active thread
    pub(crate) known_message_ids: HashSet<i32>,
    pub(crate) current_thread_id: Option<i64>,
    /// All underlying SMS thread IDs composing the currently-open
    /// `LogicalConversation`. Always contains `current_thread_id` (the primary)
    /// when a conversation is open; empty otherwise. Drives the per-thread
    /// message subscription fan-out in `subscriptions()`.
    pub(crate) current_merged_thread_ids: Vec<i64>,
    pub(crate) current_thread_addresses: Option<Vec<String>>,
    pub(crate) messages: Vec<SmsMessage>,
    pub(crate) sms_loading_state: SmsLoadingState,
    pub(crate) contacts: ContactLookup,

    // File staged for the next send
    pub(crate) pending_attachment: Option<StagedAttachment>,

    // Reply compose / send
    pub(crate) sms_compose_text: widget::text_editor::Content,
    pub(crate) sms_sending: bool,
    pub(crate) sms_sending_body: Option<String>,

    // Message pagination / scroll preservation
    pub(crate) messages_has_more: bool,
    pub(crate) older_page: Option<OlderPageLoad>,
    pub(crate) thread_has_more: HashMap<i64, bool>,
    /// Last `conversationLoaded` count per thread of the open set. Only whether it
    /// grows is used: the count includes uids the daemon's cache overwrote.
    pub(crate) last_count: HashMap<i64, u64>,

    // New-message compose
    pub(crate) new_message_recipients: Vec<(String, String)>,
    pub(crate) new_message_recipient_input: String,
    pub(crate) new_message_body: widget::text_editor::Content,
    pub(crate) new_message_pending_attachment: Option<StagedAttachment>,
    pub(crate) new_message_sending: bool,
    pub(crate) contact_suggestions: Vec<(String, String)>,

    // SMS notification deduplication. Keyed by (device_id, thread_id) because
    // thread IDs are device-local — phone A's thread 1234 and phone B's
    // thread 1234 are unrelated conversations.
    pub(crate) last_seen_sms: HashMap<(String, i64), i64>,

    // Long-press copy
    pub(crate) pressed_bubble_uid: Option<i32>,
    pub(crate) pressed_bubble_body: Option<String>,
    pub(crate) show_copy_hint: bool,
}

impl SmsConversationStore {
    pub fn new() -> Self {
        Self {
            sms_device_id: None,
            sms_device_name: None,
            raw_conversations: Vec::new(),
            conversations: Vec::new(),
            sms_prefetch: None,
            conversation_sync_active: false,
            new_message_sync_active: false,
            conversation_list_subscription_active: false,
            message_sync_active: false,
            conversation_load_active: false,
            conversation_list_subscription_generation: 0,
            conversation_message_subscription_generation: 0,
            initial_load_complete: false,
            known_message_ids: HashSet::new(),
            current_thread_id: None,
            current_merged_thread_ids: Vec::new(),
            current_thread_addresses: None,
            messages: Vec::new(),
            sms_loading_state: SmsLoadingState::Idle,
            contacts: ContactLookup::default(),
            pending_attachment: None,
            sms_compose_text: widget::text_editor::Content::new(),
            sms_sending: false,
            sms_sending_body: None,
            messages_has_more: true,
            older_page: None,
            thread_has_more: HashMap::new(),
            last_count: HashMap::new(),
            new_message_recipients: Vec::new(),
            new_message_recipient_input: String::new(),
            new_message_body: widget::text_editor::Content::new(),
            new_message_pending_attachment: None,
            new_message_sending: false,
            contact_suggestions: Vec::new(),
            last_seen_sms: HashMap::new(),
            pressed_bubble_uid: None,
            pressed_bubble_body: None,
            show_copy_hint: false,
        }
    }

    /// Check if loading more messages (pagination)
    pub(crate) fn is_loading_more_messages(&self) -> bool {
        matches!(self.sms_loading_state, SmsLoadingState::LoadingMoreMessages)
    }

    /// Reset the open thread's load bookkeeping to what a fresh first page starts
    /// from: no messages or uids, ever thread of the merged set with more, no page
    /// in flight, initial load pending. Seeds `thread_has_more` from
    /// `current_merged_thread_ids`, so set (or clear) those first. Called on open,
    /// on both closes, and on the reconnect regeneration; the view-lifecycle flags
    /// stay with each caller.
    pub(crate) fn reset_thread_state(&mut self) {
        self.messages.clear();
        self.known_message_ids.clear();
        self.messages_has_more = true;
        self.older_page = None;
        self.thread_has_more = self
            .current_merged_thread_ids
            .iter()
            .map(|&t| (t, true))
            .collect();
        self.last_count.clear();
        self.initial_load_complete = false;
    }

    /// Validate a picked path and stage it into `slot`
    fn stage_attachment(
        slot: &mut Option<StagedAttachment>,
        path: Option<std::path::PathBuf>,
    ) -> SmsReply {
        let Some(path) = path else {
            return SmsReply::NoOp;
        };
        // The daemon takes attachment paths as D-Bus strings, so they must be UTF-8.
        if path.to_str().is_none() {
            return SmsReply::Status(fl!("attachment-unreadable"));
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            return SmsReply::Status(fl!("attachment-unreadable"));
        };
        if !meta.is_file() || std::fs::File::open(&path).is_err() {
            return SmsReply::Status(fl!("attachment-unreadable"));
        }
        *slot = Some(StagedAttachment {
            path,
            size: meta.len(),
        });
        SmsReply::NoOp
    }

    /// Settle an in-flight older page: clear the spinner and recompute
    /// `messages_has_more` from the per-thread decisions `try_settle` recorded.
    fn finalize_older_page(&mut self) -> cosmic::app::Task<Message> {
        let Some(load) = self.older_page.take() else {
            return cosmic::app::Task::none();
        };
        if matches!(self.sms_loading_state, SmsLoadingState::LoadingMoreMessages) {
            self.sms_loading_state = SmsLoadingState::Idle;
        }

        // New messages this page actually added.
        let prepended = self.messages.len().saturating_sub(load.len_before);

        self.messages_has_more = self
            .current_merged_thread_ids
            .iter()
            .any(|t| *self.thread_has_more.get(t).unwrap_or(&false));

        // Records which completion detector settled this page and why pagination
        // continued or stopped — the log is otherwise silent here, so a capture
        // cannot distinguish the full-page path from the conversationLoaded one.
        tracing::debug!(
            "Older page settled: detector={:?}, requested={:?}, served={:?}, new={:?}, grew={:?}, prepended={}, messages={}, has_more={}",
            load.settled_by,
            load.requested,
            load.served,
            load.new,
            load.grew,
            prepended,
            self.messages.len(),
            self.messages_has_more
        );
        cosmic::app::Task::none()
    }

    /// Drop `thread_id` from the in-flight page, recording which detector settled
    /// it. Returns `true` when it was the last pending thread, i.e. the caller
    /// should finalize. `false` when no page is in flight
    fn settle_thread(&mut self, thread_id: i64, detector: &'static str) -> bool {
        let Some(load) = self.older_page.as_mut() else {
            return false;
        };
        load.pending.remove(&thread_id);
        load.settled_by.insert(thread_id, detector);
        load.pending.is_empty()
    }

    /// Settle `thread_id` if its page is done (see `page_settle`), finalizing the
    /// batch when it was the last pending thread.
    fn try_settle(&mut self, thread_id: i64) -> Option<cosmic::app::Task<Message>> {
        let load = self
            .older_page
            .as_ref()
            .filter(|l| l.pending.contains(&thread_id))?;
        let (has_more, detector) = page_settle(
            load.page_size,
            load.requested
                .get(&thread_id)
                .copied()
                .unwrap_or(load.page_size),
            load.served.get(&thread_id).copied().unwrap_or(0),
            load.new.get(&thread_id).copied().unwrap_or(0),
            load.grew.get(&thread_id).copied(),
        )?;
        self.thread_has_more.insert(thread_id, has_more);
        self.settle_thread(thread_id, detector)
            .then(|| self.finalize_older_page())
    }

    fn display_floor(&self) -> Option<i64> {
        display_floor(
            &self.messages,
            &self.current_merged_thread_ids,
            &self.thread_has_more,
        )
    }

    /// Fire an older page for threads at the display floor. Returns `none()`
    /// when nothing is wanted: no more, a page in flight, the initial load still
    /// running, or no messages.
    fn request_older_page(&mut self, ctx: &SmsCtx) -> cosmic::app::Task<Message> {
        if !self.messages_has_more
            || self.is_loading_more_messages()
            || !self.initial_load_complete
            || self.messages.is_empty()
        {
            return cosmic::app::Task::none();
        }
        let (Some(conn), Some(device_id)) = (ctx.conn, self.sms_device_id.as_ref()) else {
            return cosmic::app::Task::none();
        };
        let floor = self.display_floor();
        let targets: Vec<(i64, u32)> = self
            .current_merged_thread_ids
            .iter()
            .copied()
            .filter(|t| *self.thread_has_more.get(t).unwrap_or(&true))
            .filter(|&t| {
                let oldest_t = self
                    .messages
                    .iter()
                    .filter(|m| m.thread_id == t)
                    .map(|m| m.date)
                    .min();
                oldest_t.zip(floor).is_none_or(|(d, f)| d >= f)
            })
            .map(|t| {
                let loaded_t = self.messages.iter().filter(|m| m.thread_id == t).count() as u32;
                (t, loaded_t)
            })
            .collect();
        if targets.is_empty() {
            return cosmic::app::Task::none();
        }
        tracing::debug!(
            "Prefetching older messages (floor={:?}, targets={:?})",
            floor,
            targets
        );

        self.sms_loading_state = SmsLoadingState::LoadingMoreMessages;
        self.older_page = Some(OlderPageLoad {
            served: HashMap::new(),
            new: HashMap::new(),
            requested: targets
                .iter()
                .map(|&(t, loaded_t)| (t, loaded_t + MESSAGES_PER_PAGE))
                .collect(),
            grew: HashMap::new(),
            pending: targets.iter().map(|&(t, _)| t).collect(),
            page_size: MESSAGES_PER_PAGE,
            len_before: self.messages.len(),
            settled_by: HashMap::new(),
        });

        let tasks: Vec<_> = targets
            .iter()
            .map(|&(t, loaded_t)| {
                cosmic::app::Task::perform(
                    request_older_messages_async(
                        conn.clone(),
                        device_id.clone(),
                        t,
                        loaded_t,
                        MESSAGES_PER_PAGE,
                    ),
                    cosmic::Action::App,
                )
            })
            .collect();
        cosmic::app::Task::batch(tasks)
    }

    /// Re-derive `conversations` from the raw cache. Honors
    /// `config.merge_reaction_threads`: runs `merge_into_logical` when on;
    /// falls back to 1:1 `from_single` wrapping when off, and in the off
    /// case marks each entry whose underlying thread has a reaction-bucket
    /// sibling so the UI can surface "would-merge" indicators. Call after
    /// any mutation of `raw_conversations`, or after the toggle changes.
    pub(crate) fn rederive_conversations(&mut self, config: &Config) {
        self.conversations = if config.merge_reaction_threads {
            merge_into_logical(&self.raw_conversations)
        } else {
            let candidates = split_candidate_thread_ids(&self.raw_conversations);
            self.raw_conversations
                .iter()
                .cloned()
                .map(|cs| {
                    let mut lc = LogicalConversation::from_single(cs);
                    lc.is_split_candidate = candidates.contains(&lc.primary_thread_id);
                    lc
                })
                .collect()
        };
        self.conversations.sort_by_key(|lc| {
            (
                std::cmp::Reverse(lc.last_message_timestamp),
                lc.primary_thread_id,
            )
        });
        self.conversations
            .truncate(crate::constants::sms::CONVERSATION_LIST_DISPLAY_LIMIT);
    }

    /// Find the `LogicalConversation` containing `thread_id` (whether as
    /// primary or as a merged sibling). Returns `None` for unknown thread IDs.
    fn logical_for(&self, thread_id: i64) -> Option<&LogicalConversation> {
        self.conversations
            .iter()
            .find(|lc| lc.merged_thread_ids.contains(&thread_id))
    }

    /// Recipients and SIM for a reply: the newest loaded message that carries
    /// addresses, else the conversation's own address list. Newest across the whole
    /// merged set is safe because a symmetric merge has equal canonical addresses
    /// and equal `sub_id` by construction. `None` means nothing to send to.
    pub(crate) fn reply_recipients(&self, thread_id: i64) -> Option<(Vec<String>, i64)> {
        let newest_addressed = self
            .messages
            .iter()
            .filter(|m| m.uid != OPTIMISTIC_MESSAGE_UID)
            .filter_map(|m| {
                let addrs = dedupe_addresses(&m.addresses);
                (!addrs.is_empty()).then_some((m.date, addrs, m.sub_id))
            })
            .max_by_key(|(date, _, _)| *date);
        if let Some((_, addrs, sub_id)) = newest_addressed {
            return Some((addrs, sub_id));
        }
        let fallback = dedupe_addresses(self.current_thread_addresses.as_deref().unwrap_or(&[]));
        if fallback.is_empty() {
            return None;
        }
        let sub_id = self
            .logical_for(thread_id)
            .map(|lc| lc.subscription_id)
            .unwrap_or(-1);
        Some((fallback, sub_id))
    }

    /// Find the latest conversation timestamp for a phone number.
    /// Uses suffix matching (last 10 digits) to handle format variations.
    pub(crate) fn find_conversation_timestamp(&self, phone: &str) -> Option<i64> {
        let phone_digits = normalize_phone_number(phone);
        let target_suffix = phone_suffix(&phone_digits);

        self.conversations
            .iter()
            .filter(|conv| {
                conv.addresses.iter().any(|addr| {
                    let addr_digits = normalize_phone_number(addr);
                    let addr_suffix = phone_suffix(&addr_digits);
                    target_suffix == addr_suffix
                })
            })
            .map(|conv| conv.last_message_timestamp)
            .max()
    }

    /// Generate contact suggestions with phone numbers sorted by conversation recency.
    /// Returns (contact_name, phone_number) tuples, limited to max_suggestions.
    pub(crate) fn generate_contact_suggestions(
        &self,
        query: &str,
        max_suggestions: usize,
    ) -> Vec<(String, String)> {
        if query.is_empty() {
            return Vec::new();
        }

        // Search for contacts matching the query (get more to account for multi-number expansion)
        let matching_contacts = self.contacts.search_by_name(query, max_suggestions);

        // Expand each contact into (name, phone, timestamp) entries
        let mut entries: Vec<(String, String, Option<i64>)> = Vec::new();
        for contact in matching_contacts {
            for phone in &contact.phone_numbers {
                let timestamp = self.find_conversation_timestamp(phone);
                entries.push((contact.name.clone(), phone.clone(), timestamp));
            }
        }

        // Sort by timestamp: most recent conversations first, then None (never contacted)
        entries.sort_by(|a, b| match (&b.2, &a.2) {
            (Some(ts_b), Some(ts_a)) => ts_b.cmp(ts_a), // Both have timestamps: recent first
            (Some(_), None) => std::cmp::Ordering::Less, // b has timestamp, a doesn't: b first
            (None, Some(_)) => std::cmp::Ordering::Greater, // a has timestamp, b doesn't: a first
            (None, None) => std::cmp::Ordering::Equal,  // Neither has timestamp: keep order
        });

        // Dedup by phone-number suffix, keeping the most-recent entry
        // Then take up to max_suggestions and drop the timestamp
        let mut seen_numbers = HashSet::new();
        entries
            .into_iter()
            .filter(|(_, phone, _)| {
                let normalized = normalize_phone_number(phone);
                seen_numbers.insert(phone_suffix(&normalized).to_string())
            })
            .take(max_suggestions)
            .map(|(name, phone, _)| (name, phone))
            .collect()
    }

    /// Check if a phone number is already in the committed recipients list.
    /// Uses suffix matching (last 10 digits) to handle format variations.
    pub(crate) fn is_recipient_duplicate(&self, phone: &str) -> bool {
        let normalized = normalize_phone_number(phone);
        let suffix = phone_suffix(&normalized);
        self.new_message_recipients.iter().any(|(_, existing)| {
            let existing_normalized = normalize_phone_number(existing);
            phone_suffix(&existing_normalized) == suffix
        })
    }

    /// Generate contact suggestions filtered to exclude already-added recipients.
    pub(crate) fn generate_contact_suggestions_filtered(
        &self,
        query: &str,
        max: usize,
    ) -> Vec<(String, String)> {
        self.generate_contact_suggestions(query, max + self.new_message_recipients.len())
            .into_iter()
            .filter(|(_, phone)| !self.is_recipient_duplicate(phone))
            .take(max)
            .collect()
    }

    pub fn update(&mut self, msg: Message, ctx: &SmsCtx) -> (cosmic::app::Task<Message>, SmsReply) {
        match msg {
            Message::SmsPrefetchReady(device_id, conversations) => {
                if !conversations.is_empty() {
                    self.sms_prefetch = Some((device_id, conversations));
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            // Subscription-based conversation list loading handlers
            Message::ConversationReceived {
                device_id,
                conversation,
            } => {
                // Guard: Only process if for current device
                if self.sms_device_id.as_ref() != Some(&device_id) {
                    tracing::debug!(
                        "Ignoring conversation for device {} (current: {:?})",
                        device_id,
                        self.sms_device_id
                    );
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }
                // A send is only waiting for the list to change; any change concludes it.
                self.new_message_sync_active = false;

                // Upsert into raw cache by underlying thread_id. Re-derive
                // logical conversations after — incremental upsert on the
                // logical list would create duplicate groups when a new
                // thread joins an existing reaction-bucket merge.
                if let Some(existing) = self
                    .raw_conversations
                    .iter_mut()
                    .find(|cs| cs.thread_id == conversation.thread_id)
                {
                    if conversation.timestamp > existing.timestamp {
                        *existing = conversation.clone();
                        tracing::debug!(
                            "Updated conversation thread {} (newer timestamp)",
                            conversation.thread_id
                        );
                    }
                } else {
                    self.raw_conversations.push(conversation.clone());
                    tracing::debug!("Added new conversation thread {}", conversation.thread_id);
                }

                // Re-sort raw cache by timestamp (newest first).
                self.raw_conversations
                    .sort_by_key(|cs| std::cmp::Reverse(cs.timestamp));

                self.rederive_conversations(ctx.config);

                // Update last_seen for notification deduplication
                let key = (device_id, conversation.thread_id);
                let current = self.last_seen_sms.get(&key).copied();
                if current.is_none() || current < Some(conversation.timestamp) {
                    self.last_seen_sms.insert(key, conversation.timestamp);
                }

                // Transition from loading spinner to showing data (but keep sync indicator)
                if matches!(
                    self.sms_loading_state,
                    SmsLoadingState::LoadingConversations(_)
                ) {
                    self.sms_loading_state = SmsLoadingState::Idle;
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            Message::ConversationsBatchReceived {
                device_id,
                conversations,
            } => {
                if self.sms_device_id.as_ref() != Some(&device_id) {
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }
                if conversations.is_empty() {
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }

                tracing::debug!(
                    "Conversation batch: {} entries for device {}",
                    conversations.len(),
                    device_id
                );

                // A send is only waiting for the list to change; any change concludes it.
                self.new_message_sync_active = false;

                // Upsert through a map. The per-item linear 'find' in
                // ConversationReceived is fine for one conversation but
                // quadratic across a thousand-entry drain.
                let mut by_thread: HashMap<i64, ConversationSummary> = self
                    .raw_conversations
                    .drain(..)
                    .map(|cs| (cs.thread_id, cs))
                    .collect();

                for conversation in conversations {
                    let key = (device_id.clone(), conversation.thread_id);
                    let current = self.last_seen_sms.get(&key).copied();
                    if current.is_none() || current < Some(conversation.timestamp) {
                        self.last_seen_sms.insert(key, conversation.timestamp);
                    }

                    match by_thread.entry(conversation.thread_id) {
                        Entry::Occupied(mut slot) => {
                            if conversation.timestamp > slot.get().timestamp {
                                slot.insert(conversation);
                            }
                        }
                        Entry::Vacant(slot) => {
                            slot.insert(conversation);
                        }
                    }
                }

                self.raw_conversations = by_thread.into_values().collect();

                // Sort once per batch.
                // thread_id is the tiebreak so HashMap iteration order can't
                // make the list unstable between equal timestamps
                self.raw_conversations
                    .sort_by_key(|cs| (std::cmp::Reverse(cs.timestamp), cs.thread_id));

                self.rederive_conversations(ctx.config);

                if matches!(
                    self.sms_loading_state,
                    SmsLoadingState::LoadingConversations(_)
                ) {
                    self.sms_loading_state = SmsLoadingState::Idle;
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            Message::ConversationSyncStarted { device_id } => {
                // Guard: Only process if for current device
                if self.sms_device_id.as_ref() != Some(&device_id) {
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }

                tracing::debug!("Conversation sync started for device {}", device_id);
                // Update loading phase to indicate we're waiting for signals
                if matches!(
                    self.sms_loading_state,
                    SmsLoadingState::LoadingConversations(LoadingPhase::Connecting)
                ) {
                    self.sms_loading_state =
                        SmsLoadingState::LoadingConversations(LoadingPhase::Requesting);
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            Message::ConversationSyncComplete { device_id } => {
                // Guard: Only process if for current device
                if self.sms_device_id.as_ref() != Some(&device_id) {
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }

                tracing::info!(
                    "Conversation sync indicator dismissed for device {}, {} conversations loaded",
                    device_id,
                    self.conversations.len()
                );

                // Clear sync indicator only. The subscription keeps running
                // to catch new conversations while the SMS view is open.
                self.conversation_sync_active = false;

                // Dismiss the spinner even on an empty list. A device with no threads
                // sends zero packets and no end marker, so an empty result and a
                // slow phone are indistinguishable on the wire. The subscription
                // keeps listening, and a late phone's conversations replace the empty list
                if matches!(
                    self.sms_loading_state,
                    SmsLoadingState::LoadingConversations(_)
                ) {
                    self.sms_loading_state = SmsLoadingState::Idle;
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            Message::ContactsLoaded(device_id, contacts) => {
                // Only update if contacts are for the current SMS device
                if self.sms_device_id.as_ref() == Some(&device_id) {
                    tracing::info!(
                        "Loaded {} contacts for device {}",
                        contacts.len(),
                        device_id
                    );
                    self.contacts = contacts;
                } else {
                    tracing::debug!(
                        "Ignoring contacts for device {} (current: {:?})",
                        device_id,
                        self.sms_device_id
                    );
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            Message::SmsError(err) => {
                tracing::error!("SMS error: {}", err);
                self.sms_loading_state = SmsLoadingState::Idle;
                self.message_sync_active = false;
                let status = format!("SMS error: {}", err);
                (cosmic::app::Task::none(), SmsReply::Status(status))
            }

            // === Batch 2: Message thread / scroll / bubble ===
            Message::OlderMessagesRequested { thread_id, ok } => {
                if !ok {
                    // Fire failed: abort the page so the spinner clears and the user can
                    // retry by scrolling. has_more left as-is (still true), so retry works
                    tracing::warn!(
                        "Older-page request failed for thread {}, aborting page",
                        thread_id
                    );
                    if self.settle_thread(thread_id, "fire-failed") {
                        return (self.finalize_older_page(), SmsReply::NoOp);
                    }
                    if matches!(self.sms_loading_state, SmsLoadingState::LoadingMoreMessages) {
                        self.sms_loading_state = SmsLoadingState::Idle;
                    }
                }
                // ok == true; wait for streamed messages / ConversationStoreLoaded
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            Message::LoadOlderMessages => (self.request_older_page(ctx), SmsReply::NoOp),

            Message::BubblePressStarted { uid, body } => {
                self.pressed_bubble_uid = Some(uid);
                self.pressed_bubble_body = Some(body);
                self.show_copy_hint = false;
                // Spawn delayed task - fires after 500ms to show hint
                (
                    cosmic::app::Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        },
                        |_| cosmic::Action::App(Message::BubbleHintTimer),
                    ),
                    SmsReply::NoOp,
                )
            }

            Message::BubblePressReleased => {
                // Clear pressed state - cancels the long-press action
                self.pressed_bubble_uid = None;
                self.pressed_bubble_body = None;
                self.show_copy_hint = false;
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            Message::BubbleHintTimer => {
                // 500ms elapsed - show "Hold to copy" hint and start 1.5s timer for actual copy
                if self.pressed_bubble_uid.is_some() {
                    self.show_copy_hint = true;
                    return (
                        cosmic::app::Task::perform(
                            async {
                                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                            },
                            |_| cosmic::Action::App(Message::BubbleLongPressComplete),
                        ),
                        SmsReply::NoOp,
                    );
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            Message::BubbleLongPressComplete => {
                // 2s total elapsed - copy to clipboard if still pressed
                if let Some(body) = self.pressed_bubble_body.take() {
                    self.pressed_bubble_uid = None;
                    self.show_copy_hint = false;
                    return (clipboard::write(body), SmsReply::NoOp);
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            // Subscription-based message loading handlers
            Message::ConversationLoadStarted { thread_id } => {
                // D-Bus request fired, subscription is now active.
                // Accept signals from any thread in the open merged set so
                // every fanned-out subscription can flip the loading phase.
                if self.current_merged_thread_ids.contains(&thread_id) {
                    tracing::debug!(
                        "Conversation {} load started, waiting for subscription signals",
                        thread_id
                    );
                    // Update loading phase to indicate we're waiting for signals
                    if matches!(
                        self.sms_loading_state,
                        SmsLoadingState::LoadingMessages(LoadingPhase::Connecting)
                    ) {
                        self.sms_loading_state =
                            SmsLoadingState::LoadingMessages(LoadingPhase::Requesting);
                    }
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::ConversationMessageReceived { thread_id, message } => {
                // Guard: accept messages from any thread in the open merged set.
                // Reactions split into bucket threads arrive on a different
                // threadId than the primary; rejecting them here would
                // re-introduce the orphaned-reactions UX bug.
                if !self.current_merged_thread_ids.contains(&thread_id) {
                    tracing::debug!(
                        "Ignoring message for thread {} (open merged set: {:?})",
                        thread_id,
                        self.current_merged_thread_ids
                    );
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }
                let message_thread_id = message.thread_id;
                // Count daemon-served messages toward an in-flight older page: pre-dedup, so
                // it is the position in the range (the daemon's numHandled), not progress.
                if let Some(load) = self.older_page.as_mut() {
                    *load.served.entry(message.thread_id).or_insert(0) += 1;
                }

                // Reconcile optimistic message: if this incoming sent message
                // matches our optimistic insert's body within a 5-minute window,
                // upgrade the optimistic entry in-place instead of inserting a duplicate.
                if message.uid != OPTIMISTIC_MESSAGE_UID
                    && message.message_type == MessageType::Sent
                {
                    if let Some(pos) = self.messages.iter().position(|m| {
                        m.uid == OPTIMISTIC_MESSAGE_UID
                            && m.message_type == MessageType::Sent
                            && m.body == message.body
                            && (message.date - m.date).unsigned_abs() < 300_000
                    }) {
                        tracing::info!(
                            "Reconciling optimistic message with real uid={}",
                            message.uid
                        );
                        self.messages[pos].uid = message.uid;
                        self.messages[pos].date = message.date;
                        // The optimistic entry was inserted without attachments (we do not build a
                        // local preview); the phone's echo is the authoritative copy, so adopt its
                        // attachment list. Without this an attachment send reconciles cleanly and
                        // then renders forever without its image.
                        self.messages[pos].attachments = message.attachments.clone();
                        self.known_message_ids.remove(&OPTIMISTIC_MESSAGE_UID);
                        self.known_message_ids.insert(message.uid);
                        self.sms_sending_body = None;
                        return (
                            self.try_settle(message_thread_id)
                                .unwrap_or_else(cosmic::app::Task::none),
                            SmsReply::NoOp,
                        );
                    }
                }

                // Deduplication: skip if already have this message
                if self.known_message_ids.contains(&message.uid) {
                    tracing::debug!(
                        "Skipping duplicate message uid={} for thread {}",
                        message.uid,
                        thread_id
                    );
                    return (
                        self.try_settle(message_thread_id)
                            .unwrap_or_else(cosmic::app::Task::none),
                        SmsReply::NoOp,
                    );
                }
                self.known_message_ids.insert(message.uid);

                // Check if this confirms our pending sent message
                let confirmed_send = self.sms_sending_body.is_some()
                    && message.message_type == MessageType::Sent
                    && self.sms_sending_body.as_deref() == Some(message.body.as_str());
                if confirmed_send {
                    tracing::info!("Confirmed delivery of sent message uid={}", message.uid);
                    self.sms_sending_body = None;
                }

                // Insert message in sorted order by date
                let insert_pos = self
                    .messages
                    .iter()
                    .position(|m| m.date > message.date)
                    .unwrap_or(self.messages.len());
                self.messages.insert(insert_pos, message);
                if let Some(load) = self.older_page.as_mut() {
                    *load.new.entry(message_thread_id).or_insert(0) += 1;
                }
                // Every exit below carries this, so an echo or a confirmed send landing as a
                // page's last signal still settles it.
                let settle = self
                    .try_settle(message_thread_id)
                    .unwrap_or_else(cosmic::app::Task::none);

                tracing::debug!(
                    "Added message to thread {}, now have {} messages",
                    thread_id,
                    self.messages.len()
                );

                // Clear loading spinner after first message, show sync indicator instead
                if matches!(self.sms_loading_state, SmsLoadingState::LoadingMessages(_)) {
                    self.sms_loading_state = SmsLoadingState::Idle;
                    self.message_sync_active = true;
                }

                // Scroll to bottom when a sent message is confirmed.
                if confirmed_send {
                    return (
                        cosmic::app::Task::batch([
                            settle,
                            scrollable::snap_to(
                                widget::Id::new("message-thread"),
                                scrollable::RelativeOffset::START.into(),
                            ),
                        ]),
                        SmsReply::NoOp,
                    );
                }
                // While the initial load is in flight, keep the newest message
                // in view as messages stream in. Necessary because the daemon's
                // worker emits `conversationLoaded` only when it actually
                // fetched fresh phone data (see daemon's
                // `addMessages()` → `conversationLoaded`); for cache-served
                // hits the worker just emits per-message signals and finishes
                // silently, so `ConversationStoreLoaded` and
                // `ConversationLoadComplete` never fire and the scroll stays
                // pinned at the top of an oldest-first list. Bounded by
                // `initial_load_complete` so we don't yank a user reading
                // older messages when a new SMS arrives later.
                if !self.initial_load_complete {
                    return (
                        cosmic::app::Task::batch([
                            settle,
                            scrollable::snap_to(
                                widget::Id::new("message-thread"),
                                scrollable::RelativeOffset::START.into(),
                            ),
                        ]),
                        SmsReply::NoOp,
                    );
                }
                (settle, SmsReply::NoOp)
            }
            Message::ConversationStoreLoaded {
                thread_id,
                total_count,
            } => {
                // `conversationLoaded` arrived - scroll to show messages while the page (on
                // a fetch) and any later phone data stream in. Each fanned-out subscription
                // emits its own ConversationStoreLoaded; accept any signal whose thread is
                // in the open merged set.
                if !self.current_merged_thread_ids.contains(&thread_id) {
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }

                let grew = total_count > 0
                    && self
                        .last_count
                        .get(&thread_id)
                        .is_none_or(|&prev| total_count > prev);
                if total_count > 0 {
                    self.last_count.insert(thread_id, total_count);
                }

                // During an older page this records whether the count grew; the thread
                // settles on the first conversationLoaded after the cache burst (see
                // `page_settle`).
                if let Some(load) = self
                    .older_page
                    .as_mut()
                    .filter(|l| l.pending.contains(&thread_id))
                {
                    // Before the cache burst (`served == 0`) this is another request's signal
                    if total_count > 0 && load.served.get(&thread_id).is_some_and(|&s| s > 0) {
                        *load.grew.entry(thread_id).or_insert(false) |= grew;
                    }
                    return (
                        self.try_settle(thread_id)
                            .unwrap_or_else(cosmic::app::Task::none),
                        SmsReply::NoOp,
                    );
                }
                tracing::info!(
                    "conversationLoaded for thread {}: {} messages displayed across the merged set, daemon holds {} for this thread",
                    thread_id,
                    self.messages.len(),
                    total_count
                );

                // Update pagination state via helper (handles merged-set math).
                let loaded_t = self
                    .messages
                    .iter()
                    .filter(|m| m.thread_id == thread_id)
                    .count() as u64;
                // `total_count == 0` on the collected-reply path means "unknown",
                // not "empty" (daemon quirk) — fall back to the per-thread page
                // heuristic so a late-arriving 0 can't latch has_more false and
                // kill the first scroll's prefetch.
                let has_more_t = if total_count > 0 {
                    loaded_t < total_count
                } else {
                    loaded_t >= MESSAGES_PER_PAGE as u64
                };
                // Never raises: outside a page a grown count is usually a new incoming
                // message, not older history, and raising would bring back the floor.
                let has_more_t =
                    has_more_t && *self.thread_has_more.get(&thread_id).unwrap_or(&true);
                self.thread_has_more.insert(thread_id, has_more_t);
                self.messages_has_more = self
                    .current_merged_thread_ids
                    .iter()
                    .any(|t| *self.thread_has_more.get(t).unwrap_or(&false));

                // Scroll to bottom to show latest messages
                if !self.initial_load_complete && !self.messages.is_empty() {
                    return (
                        scrollable::snap_to(
                            widget::Id::new("message-thread"),
                            scrollable::RelativeOffset::START.into(),
                        ),
                        SmsReply::NoOp,
                    );
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::ConversationLoadComplete {
                thread_id,
                total_count,
            } => {
                // Guard: accept completion signal from any thread in the open
                // merged set. Each fanned-out subscription emits its own
                // ConversationLoadComplete
                if !self.current_merged_thread_ids.contains(&thread_id) {
                    tracing::debug!(
                        "Ignoring load complete for thread {} (open merged set: {:?})",
                        thread_id,
                        self.current_merged_thread_ids
                    );
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }

                let was_already_complete = self.initial_load_complete;

                tracing::info!(
                    "Conversation {} loading complete: {} messages loaded, {} total in conversation \
                     (already_complete={})",
                    thread_id,
                    self.messages.len(),
                    total_count,
                    was_already_complete
                );

                // Idempotent state refresh — safe to redo on each completion in
                // a merged set (one ConversationLoadComplete per fanned-out
                // subscription). Sort + pagination math + last_seen_sms all
                // converge on the same final values regardless of arrival order.
                self.messages.sort_by_key(|m| m.date);
                let loaded_t = self
                    .messages
                    .iter()
                    .filter(|m| m.thread_id == thread_id)
                    .count() as u64;
                // `total_count == 0` on the collected-reply path means "unknown",
                // not "empty" (daemon quirk) — fall back to the per-thread page
                // heuristic so a late-arriving 0 can't latch has_more false and
                // kill the first scroll's prefetch.
                let has_more_t = if total_count > 0 {
                    loaded_t < total_count
                } else {
                    loaded_t >= MESSAGES_PER_PAGE as u64
                };
                self.thread_has_more.insert(thread_id, has_more_t);
                self.messages_has_more = self
                    .current_merged_thread_ids
                    .iter()
                    .any(|t| *self.thread_has_more.get(t).unwrap_or(&false));
                if let Some(newest) = self.messages.iter().map(|m| m.date).max() {
                    if let Some(device_id) = self.sms_device_id.clone() {
                        let key = (device_id, thread_id);
                        let current = self.last_seen_sms.get(&key).copied();
                        if current.is_none() || current < Some(newest) {
                            self.last_seen_sms.insert(key, newest);
                        }
                    }
                }

                // First-completion-only effects: clear loading indicators and
                // snap to the latest message. Skipping these on repeat
                // arrivals avoids yanking the user back to the bottom if a
                // late-completing subscription fires after they've started
                // scrolling. Note: subscriptions keep running to catch new
                // messages (including sent-message echoes).
                if was_already_complete {
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }

                self.message_sync_active = false;
                self.initial_load_complete = true;
                self.sms_loading_state = SmsLoadingState::Idle;

                if !self.messages.is_empty() {
                    return (
                        scrollable::snap_to(
                            widget::Id::new("message-thread"),
                            scrollable::RelativeOffset::START.into(),
                        ),
                        SmsReply::NoOp,
                    );
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }

            // === Batch 3: Reply send + attachments + notification ===
            Message::SmsComposeAction(action) => {
                self.sms_compose_text.perform(action);
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::SmsAttachmentSelected((device_id, thread_id), path) => {
                if self.sms_device_id.as_deref() != Some(device_id.as_str())
                    || self.current_thread_id != Some(thread_id)
                {
                    tracing::debug!(
                        "attachment picked for thread {} dropped: no longer open",
                        thread_id
                    );
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }
                let reply = Self::stage_attachment(&mut self.pending_attachment, path);
                (cosmic::app::Task::none(), reply)
            }
            Message::ClearSmsAttachment => {
                self.pending_attachment = None;
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::NewMessageAttachmentSelected(path) => {
                let reply = Self::stage_attachment(&mut self.new_message_pending_attachment, path);
                (cosmic::app::Task::none(), reply)
            }
            Message::ClearNewMessageAttachment => {
                self.new_message_pending_attachment = None;
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::SendSms => {
                tracing::debug!(
                    "State: conn={}, device_id={:?}, thread_id={:?}, text_empty={}, sending={}",
                    ctx.conn.is_some(),
                    self.sms_device_id,
                    self.current_thread_id,
                    self.sms_compose_text.text().trim().is_empty(),
                    self.sms_sending
                );
                if let (Some(conn), Some(device_id), Some(thread_id)) = (
                    ctx.conn,
                    self.sms_device_id.as_ref(),
                    self.current_thread_id,
                ) {
                    let message_text = self.sms_compose_text.text();
                    let has_content =
                        !message_text.trim().is_empty() || self.pending_attachment.is_some();
                    let too_large = self
                        .pending_attachment
                        .as_ref()
                        .is_some_and(|a| a.too_large(&message_text));
                    if has_content && !too_large && !self.sms_sending {
                        let Some((addresses, sub_id)) = self.reply_recipients(thread_id) else {
                            tracing::warn!("No recipient for thread {}; nothing sent", thread_id);
                            return (
                                cosmic::app::Task::none(),
                                SmsReply::Status(fl!("reply-no-recipient")),
                            );
                        };
                        self.sms_sending = true;
                        self.sms_sending_body = Some(message_text.clone());
                        tracing::info!(
                            "Dispatching sendSms for thread {} to {} recipient(s), sub_id {}",
                            thread_id,
                            addresses.len(),
                            sub_id
                        );
                        return (
                            cosmic::app::Task::perform(
                                send_sms_async(
                                    conn.clone(),
                                    device_id.clone(),
                                    addresses,
                                    sub_id,
                                    message_text,
                                    self.pending_attachment.as_ref().map(|a| a.path.clone()),
                                ),
                                cosmic::Action::App,
                            ),
                            SmsReply::NoOp,
                        );
                    } else {
                        tracing::warn!(
                            "SendSms conditions not met: text_empty={}, has_attachment={}, sending={}, too_large={}",
                            message_text.trim().is_empty(),
                            self.pending_attachment.is_some(),
                            self.sms_sending,
                            too_large
                        );
                    }
                } else {
                    tracing::warn!("SendSms missing required state");
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::SmsSendResult(result) => {
                self.sms_sending = false;
                match result {
                    Ok(sent_body) => {
                        tracing::debug!("SMS sent successfully");
                        self.sms_compose_text = widget::text_editor::Content::new();
                        self.pending_attachment = None;
                        if let Some(thread_id) = self.current_thread_id {
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or(0);

                            // Insert optimistic message if echo hasn't already arrived.
                            // sms_sending_body is cleared by confirmed_send in
                            // ConversationMessageReceived if the echo arrived before
                            // SmsSendResult — skip to avoid duplicate.
                            if self.sms_sending_body.is_some() {
                                // Source sub_id from the LogicalConversation
                                // populated at fetch time. The value is the
                                // same SIM property; sourcing it here closes
                                // the pre-first-message window where the
                                // optimistic stamp would have fallen back to -1.
                                let sub_id = self
                                    .logical_for(thread_id)
                                    .map(|lc| lc.subscription_id)
                                    .unwrap_or(-1);
                                let optimistic = SmsMessage {
                                    body: sent_body,
                                    addresses: self
                                        .current_thread_addresses
                                        .clone()
                                        .unwrap_or_default(),
                                    date: now_ms,
                                    message_type: MessageType::Sent,
                                    read: true,
                                    thread_id,
                                    uid: OPTIMISTIC_MESSAGE_UID,
                                    sub_id,
                                    attachments: vec![],
                                };
                                self.messages.push(optimistic);
                                self.known_message_ids.insert(OPTIMISTIC_MESSAGE_UID);
                                self.sms_sending_body = None;

                                // No subscription restart needed — the message subscription
                                // runs as long as the thread is open and will catch the
                                // phone's echo naturally for optimistic reconciliation.

                                return (
                                    scrollable::snap_to(
                                        widget::Id::new("message-thread"),
                                        scrollable::RelativeOffset::START.into(),
                                    ),
                                    SmsReply::NoOp,
                                );
                            }
                        }

                        (cosmic::app::Task::none(), SmsReply::Status(fl!("sms-sent")))
                    }
                    Err(err) => {
                        tracing::error!("SMS send error: {}", err);
                        self.sms_sending_body = None;
                        let status = format!("{}: {}", fl!("sms-failed"), err);
                        (cosmic::app::Task::none(), SmsReply::Status(status))
                    }
                }
            }

            // Attachment messages
            Message::OpenAttachment {
                device_id,
                device_name,
                part_id,
                unique_identifier,
            } => {
                // Check if KDE Connect has already cached this attachment
                // KDE Connect daemon caches to ~/.cache/kdeconnect.daemon/<device-name>/
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                let cache_dir = std::path::PathBuf::from(home)
                    .join(".cache/kdeconnect.daemon")
                    .join(&device_name);
                let cached_path = cache_dir.join(&unique_identifier);

                if cached_path.exists() {
                    // Already cached — open immediately
                    let path_str = cached_path.to_string_lossy().to_string();
                    return (
                        cosmic::Task::future(async move {
                            let _ = tokio::process::Command::new("xdg-open")
                                .arg(&path_str)
                                .spawn();
                        })
                        .discard(),
                        SmsReply::NoOp,
                    );
                }

                // Not cached — request from phone via D-Bus
                if let Some(conn) = ctx.conn {
                    return (
                        cosmic::app::Task::perform(
                            request_attachment_async(
                                conn.clone(),
                                device_id,
                                device_name,
                                part_id,
                                unique_identifier,
                            ),
                            cosmic::Action::App,
                        ),
                        SmsReply::SetStatus(Some(fl!("loading-attachment"))),
                    );
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::AttachmentReady(file_path) => (
                cosmic::Task::future(async move {
                    let _ = tokio::process::Command::new("xdg-open")
                        .arg(&file_path)
                        .spawn();
                })
                .discard(),
                SmsReply::SetStatus(None),
            ),
            Message::AttachmentError(err) => {
                tracing::error!("Attachment error: {}", err);
                (
                    cosmic::app::Task::none(),
                    SmsReply::Status(fl!("attachment-failed")),
                )
            }

            Message::SmsNotificationReceived(device_id, message) => {
                // Freshness check: only notify for messages received within the last 30 seconds.
                // This prevents false notifications when fetching historical messages and handles
                // cross-process deduplication (COSMIC spawns multiple applet instances).
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let message_age_ms = now_ms - message.date;
                if message_age_ms > 30_000 {
                    // Message is older than 30 seconds, skip notification
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }

                // Check if we've already seen this message (deduplication)
                let key = (device_id.clone(), message.thread_id);
                let last_seen = self.last_seen_sms.get(&key).copied();
                if last_seen.is_some() && last_seen >= Some(message.date) {
                    // Already seen this message or an older one
                    return (cosmic::app::Task::none(), SmsReply::NoOp);
                }

                // Update last seen timestamp for this thread
                self.last_seen_sms.insert(key, message.date);

                // Capture config settings
                let show_sender = ctx.config.sms_notification_show_sender;
                let show_content = ctx.config.sms_notification_show_content;
                let message_body = message.body.clone();

                // Resolve sender name: use cached contacts if available, otherwise load from disk
                let cached_sender_name = if show_sender {
                    let has_cached_contacts = self.sms_device_id.as_ref() == Some(&device_id)
                        && !self.contacts.is_empty();
                    if has_cached_contacts {
                        Some(self.contacts.get_group_display_name(&message.addresses, 3))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let addresses = message.addresses.clone();

                // Show notification asynchronously
                (
                    cosmic::app::Task::perform(
                        async move {
                            // Build summary: resolve sender name if needed and not already cached
                            let summary = if show_sender {
                                let sender_name = match cached_sender_name {
                                    Some(name) => name,
                                    None => {
                                        let contacts =
                                            ContactLookup::load_for_device(&device_id).await;
                                        contacts.get_group_display_name(&addresses, 3)
                                    }
                                };
                                fl!("sms-notification-title-from", sender = sender_name)
                            } else {
                                fl!("sms-notification-title")
                            };

                            let body = if show_content {
                                message_body
                            } else {
                                fl!("sms-notification-body-hidden")
                            };

                            let mut notification = notify_rust::Notification::new();
                            notification
                                .summary(&summary)
                                .body(&body)
                                .icon("phone-symbolic")
                                .appname("Connected")
                                .timeout(notify_rust::Timeout::Milliseconds(
                                    NORMAL_NOTIFICATION_TIMEOUT_MS,
                                ));
                            match tokio::task::spawn_blocking(move || notification.show()).await {
                                Ok(Ok(_handle)) => tracing::debug!("SMS notification shown"),
                                Ok(Err(e)) => {
                                    tracing::warn!("Failed to show SMS notification: {}", e)
                                }
                                Err(e) => tracing::warn!("SMS notification task panicked: {}", e),
                            }
                        },
                        |_| cosmic::Action::App(Message::RefreshDevices),
                    ),
                    SmsReply::NoOp,
                )
            }

            // === Batch 4: New-message compose ===
            Message::NewMessageRecipientInput(text) => {
                self.contact_suggestions = self.generate_contact_suggestions_filtered(&text, 10);
                self.new_message_recipient_input = text;
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::NewMessageBodyAction(action) => {
                self.new_message_body.perform(action);
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::AddManualRecipient => {
                let input = self.new_message_recipient_input.trim().to_string();
                if is_address_valid(&input) && !self.is_recipient_duplicate(&input) {
                    let display = self.contacts.get_name_or_number(&input);
                    self.new_message_recipients.push((display, input));
                    self.new_message_recipient_input.clear();
                    self.contact_suggestions.clear();
                    return (
                        widget::text_input::focus(widget::Id::new("new-message-recipient")),
                        SmsReply::NoOp,
                    );
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::RemoveRecipient(index) => {
                if index < self.new_message_recipients.len() {
                    self.new_message_recipients.remove(index);
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::SelectContact(_name, phone) => {
                if !self.is_recipient_duplicate(&phone) {
                    let display = self.contacts.get_name_or_number(&phone);
                    self.new_message_recipients.push((display, phone));
                    self.new_message_recipient_input.clear();
                    self.contact_suggestions.clear();
                    return (
                        widget::text_input::focus(widget::Id::new("new-message-recipient")),
                        SmsReply::NoOp,
                    );
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::SendNewMessage => {
                if let (Some(conn), Some(device_id)) = (ctx.conn, self.sms_device_id.as_ref()) {
                    let body_text = self.new_message_body.text();
                    let has_content = !body_text.trim().is_empty()
                        || self.new_message_pending_attachment.is_some();
                    let too_large = self
                        .new_message_pending_attachment
                        .as_ref()
                        .is_some_and(|a| a.too_large(&body_text));
                    if !self.new_message_recipients.is_empty()
                        && has_content
                        && !too_large
                        && !self.new_message_sending
                    {
                        let recipients: Vec<String> = self
                            .new_message_recipients
                            .iter()
                            .map(|(_, phone)| phone.clone())
                            .collect();
                        let message = body_text;
                        self.new_message_sending = true;
                        tracing::info!(
                            "Dispatching send_new_sms_async recipients={} has_attachment={}",
                            recipients.len(),
                            self.new_message_pending_attachment.is_some()
                        );
                        return (
                            cosmic::app::Task::perform(
                                send_new_sms_async(
                                    conn.clone(),
                                    device_id.clone(),
                                    recipients,
                                    message,
                                    self.new_message_pending_attachment
                                        .as_ref()
                                        .map(|a| a.path.clone()),
                                ),
                                cosmic::Action::App,
                            ),
                            SmsReply::NoOp,
                        );
                    } else {
                        tracing::warn!(
                            "SendNewMessage conditions not met: recipients={}, body_empty={}, has_attachment={}, sending={}, too_large={}",
                            self.new_message_recipients.len(),
                            body_text.trim().is_empty(),
                            self.new_message_pending_attachment.is_some(),
                            self.new_message_sending,
                            too_large
                        );
                    }
                } else {
                    tracing::warn!("SendNewMessage missing required state");
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::ConversationListStreamEnded { device_id } => {
                // Only regenerate if the view still wants this device's list. If the
                // user closed SMS or switched devices, do nothing.
                if self.conversation_list_subscription_active
                    && self.sms_device_id.as_deref() == Some(device_id.as_str())
                {
                    self.conversation_list_subscription_generation = self
                        .conversation_list_subscription_generation
                        .wrapping_add(1);
                    tracing::info!(
                        "Conversation-list stream ended for {}, regenerating subscription (gen{})",
                        device_id,
                        self.conversation_list_subscription_generation
                    );
                }
                // Never leave the sync spinner stuck if the stream died mid-sync.
                if matches!(
                    self.sms_loading_state,
                    SmsLoadingState::LoadingConversations(_)
                ) {
                    self.sms_loading_state = SmsLoadingState::Idle;
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::NewMessageSyncSettle { device_id } => {
                if self.sms_device_id.as_deref() == Some(device_id.as_str()) {
                    self.new_message_sync_active = false;
                }
                (cosmic::app::Task::none(), SmsReply::NoOp)
            }
            Message::NewMessageSendResult(result) => {
                self.new_message_sending = false;
                match &result {
                    Ok(msg) => {
                        tracing::info!("New message send result: {}", msg);
                        // Clear fields and return to conversation list
                        let success_msg = msg.clone();
                        self.new_message_recipients.clear();
                        self.new_message_recipient_input.clear();
                        self.new_message_pending_attachment = None;
                        self.new_message_body = widget::text_editor::Content::new();
                        // Enable subscription to catch the new conversation when the phone
                        // syncs back. The subscription listens over a longer window than a
                        // one-shot fetch, giving the phone time to process the send and
                        // emit a conversationCreated signal.
                        let mut settle_task = cosmic::app::Task::none();
                        if let Some(device_id) = self.sms_device_id.clone() {
                            self.conversation_list_subscription_active = true;
                            self.new_message_sync_active = true;
                            settle_task = cosmic::app::Task::perform(
                                async move {
                                    tokio::time::sleep(std::time::Duration::from_secs(
                                        NEW_MESSAGE_SYNC_INDICATOR_SECS,
                                    ))
                                    .await;
                                    Message::NewMessageSyncSettle { device_id }
                                },
                                cosmic::Action::App,
                            );
                        }
                        (settle_task, SmsReply::NewMessageSent(success_msg))
                    }
                    Err(err) => {
                        tracing::error!("New message send error: {}", err);
                        (
                            cosmic::app::Task::none(),
                            SmsReply::Status(format!("Send failed: {}", err)),
                        )
                    }
                }
            }

            // Every SMS variant is handled above; this arm only fires if a
            // non-SMS Message is mis-routed here from `app.rs::update`.
            _ => unreachable!("non-SMS Message routed to SmsConversationStore"),
        }
    }

    /// Render the active SMS sub-view.
    ///
    /// `status_message` is owned by the parent app and threaded through for
    /// the message-thread view's send-confirmation/error banner. `config`
    /// supplies `merge_reaction_threads` to the conversation-list view so
    /// the header toggle can show its current state.
    pub fn view<'a>(
        &'a self,
        mode: SmsViewMode,
        config: &'a Config,
        status_message: Option<&'a str>,
    ) -> Element<'a, Message> {
        match mode {
            SmsViewMode::ConversationList => view_conversation_list(ConversationListParams {
                device_name: self.sms_device_name.as_deref(),
                conversations: &self.conversations,
                contacts: &self.contacts,
                loading_state: &self.sms_loading_state,
                sync_active: self.conversation_sync_active || self.new_message_sync_active,
                merge_reaction_threads: config.merge_reaction_threads,
            }),
            SmsViewMode::MessageThread => {
                let visible_from = self
                    .display_floor()
                    .map_or(0, |floor| self.messages.partition_point(|m| m.date < floor));
                let thread = view_message_thread(MessageThreadParams {
                    device_id: self.sms_device_id.as_deref().unwrap_or(""),
                    device_name: self.sms_device_name.as_deref().unwrap_or(""),
                    thread_addresses: self.current_thread_addresses.as_deref(),
                    messages: &self.messages[visible_from..],
                    contacts: &self.contacts,
                    loading_state: &self.sms_loading_state,
                    sms_compose_text: &self.sms_compose_text,
                    pending_attachment: self.pending_attachment.as_ref(),
                    sms_sending: self.sms_sending,
                    sync_active: self.message_sync_active,
                    pressed_bubble_uid: self.pressed_bubble_uid,
                    show_copy_hint: self.show_copy_hint,
                    status_message,
                    can_load_older: self.messages_has_more && self.initial_load_complete,
                });
                // popup_container uses Shrink height internally, which sets a
                // compression flag on iced's layout limits. Under compression,
                // the flex layout processes all children in document order and a
                // scrollable's intrinsic content size consumes all available
                // height, leaving 0 for the compose row below it. A Fixed height
                // wrapper is the only way to clear that flag (Fill doesn't);
                // the value is clamped to the autosize limit, which `view_window`
                // lowers to the output's height
                widget::container(thread)
                    .height(cosmic::iced::Length::Fixed(10_000.0))
                    .width(cosmic::iced::Length::Fill)
                    .into()
            }
            SmsViewMode::NewMessage => view_new_message(NewMessageParams {
                recipients: &self.new_message_recipients,
                recipient_input: &self.new_message_recipient_input,
                body: &self.new_message_body,
                sending: self.new_message_sending,
                contact_suggestions: &self.contact_suggestions,
                pending_attachment: self.new_message_pending_attachment.as_ref(),
            }),
        }
    }

    /// SMS-state-driven subscriptions: conversation-list refresh and the
    /// per-thread message subscription. The unconditional SMS/call notification
    /// subscriptions stay in `app.rs::subscription()` because they're gated on
    /// device reachability + config, not store state.
    pub fn subscriptions(&self) -> Vec<Subscription<Message>> {
        let mut subs: Vec<Subscription<Message>> = Vec::new();

        // Conversation list subscription (incremental loading)
        if self.conversation_list_subscription_active {
            if let Some(device_id) = self.sms_device_id.clone() {
                let generation = self.conversation_list_subscription_generation;
                subs.push(Subscription::run_with(
                    ("conversation_list", device_id.clone(), generation),
                    |(_, device_id, _generation)| conversation_list_subscription(device_id.clone()),
                ));
            }
        }

        // Per-thread message subscription (incremental message loading).
        // Fans out one subscription per underlying thread in the open
        // `LogicalConversation` so reactions split into bucket threads load
        // alongside the primary. iced keys subscriptions on the id tuple, so
        // distinct `thread_id` values produce distinct running subscriptions.
        if self.conversation_load_active {
            if let Some(device_id) = self.sms_device_id.clone() {
                let messages_per_page = MESSAGES_PER_PAGE;
                let generation = self.conversation_message_subscription_generation;
                for &thread_id in &self.current_merged_thread_ids {
                    subs.push(Subscription::run_with(
                        (
                            "conversation_messages",
                            thread_id,
                            device_id.clone(),
                            messages_per_page,
                            generation,
                        ),
                        |(_, thread_id, device_id, messages_per_page, _generation)| {
                            conversation_message_subscription(
                                *thread_id,
                                device_id.clone(),
                                *messages_per_page,
                            )
                        },
                    ));
                }
            }
        }

        subs
    }
}

impl Default for SmsConversationStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_settle_rules() {
        // (requested, served, new, grew)
        type Input = (u32, u32, u32, Option<bool>);
        // page 10
        let cases: &[(Input, Option<(bool, &str)>)] = &[
            ((20, 20, 10, None), Some((true, "full-page"))),
            // duplicates only, no signal: no settle, no latch
            ((20, 20, 0, None), None),
            // short page, signal not yet arrived
            ((50, 49, 9, None), None),
            // 1291: top of history, the fetch added nothing
            (
                (110, 101, 1, Some(false)),
                Some((false, "conversation-loaded")),
            ),
            // 652: short page behind a top-up, the fetch added uids
            ((50, 49, 9, Some(true)), Some((true, "conversation-loaded"))),
            // cache filled the request, mostly duplicates
            (
                (40, 40, 5, Some(false)),
                Some((true, "conversation-loaded")),
            ),
            // a full page wins over the signal
            ((30, 30, 10, Some(false)), Some((true, "full-page"))),
        ];
        for &((requested, served, new, grew), want) in cases {
            assert_eq!(
                page_settle(10, requested, served, new, grew),
                want,
                "requested={requested} served={served} new={new} grew={grew:?}"
            );
        }
    }
    #[test]
    fn display_floor_rules() {
        // thread 1 is busy (its page covers a short recent span), thread 2 is
        // quiet (its page reaches far back)
        let at = |thread_id: i64, date: i64| SmsMessage {
            thread_id,
            ..msg(0, date, &[], -1)
        };
        let messages = [at(2, 100), at(2, 500), at(1, 900), at(1, 950)];
        let more = HashMap::from([(1, true), (2, true)]);

        // single thread: its own oldest
        assert_eq!(display_floor(&messages, &[1], &more), Some(900));
        // busy plus quiet: the busy thread's oldest
        assert_eq!(display_floor(&messages, &[1, 2], &more), Some(900));
        // busy thread at the top of its history drops out; the floor falls to the quiet one
        let busy_done = HashMap::from([(1, false), (2, true)]);
        assert_eq!(display_floor(&messages, &[1, 2], &busy_done), Some(100));
        // absent from the map counts as more
        assert_eq!(
            display_floor(&messages, &[1, 2], &HashMap::new()),
            Some(900)
        );
        // no thread with more
        let all_done = HashMap::from([(1, false), (2, false)]);
        assert_eq!(display_floor(&messages, &[1, 2], &all_done), None);
        // a thread with nothing loaded contributes nothing
        assert_eq!(display_floor(&messages, &[1, 2, 3], &more), Some(900));
    }

    fn addrs(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    fn msg(uid: i32, date: i64, addresses: &[&str], sub_id: i64) -> SmsMessage {
        SmsMessage {
            body: String::new(),
            addresses: addrs(addresses),
            date,
            message_type: MessageType::Inbox,
            read: true,
            thread_id: 1,
            uid,
            sub_id,
            attachments: vec![],
        }
    }

    #[test]
    fn test_reply_recipients_newest_addressed_wins() {
        let mut store = SmsConversationStore::new();
        store.messages = vec![
            msg(1, 100, &["+15551234567"], 2),
            msg(2, 200, &["+15559876543"], 3),
            msg(3, 300, &[], 4),
        ];
        assert_eq!(
            store.reply_recipients(1),
            Some((addrs(&["+15559876543"]), 3))
        );
    }

    #[test]
    fn test_reply_recipients_falls_back_to_conversation_addresses() {
        let mut store = SmsConversationStore::new();
        store.current_thread_addresses = Some(addrs(&["+15551234567"]));
        assert_eq!(
            store.reply_recipients(1),
            Some((addrs(&["+15551234567"]), -1))
        );
    }

    #[test]
    fn test_reply_recipients_none_when_nothing_addressed() {
        let mut store = SmsConversationStore::new();
        store.messages = vec![msg(1, 100, &[], 2)];
        assert_eq!(store.reply_recipients(1), None);
    }

    #[test]
    fn test_reply_recipients_drops_phantom() {
        let mut store = SmsConversationStore::new();
        store.messages = vec![msg(1, 100, &["0001", "+15551234567"], 2)];
        assert_eq!(
            store.reply_recipients(1),
            Some((addrs(&["+15551234567"]), 2))
        );
    }

    #[test]
    fn test_reply_recipients_skips_optimistic() {
        let mut store = SmsConversationStore::new();
        store.messages = vec![
            msg(1, 100, &["+15551234567"], 2),
            msg(OPTIMISTIC_MESSAGE_UID, 200, &["+15559876543"], 3),
        ];
        assert_eq!(
            store.reply_recipients(1),
            Some((addrs(&["+15551234567"]), 2))
        );
    }

    #[test]
    fn stage_attachment_refuses_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;
        let path =
            std::env::temp_dir().join(format!("connected-unreadable-{}", std::process::id()));
        std::fs::write(&path, b"x").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut slot = None;
        let reply = SmsConversationStore::stage_attachment(&mut slot, Some(path.clone()));
        let _ = std::fs::remove_file(&path);
        assert!(matches!(reply, SmsReply::Status(_)));
        assert!(slot.is_none());
    }
}
