package app.lentil.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.lentil.data.Author
import app.lentil.data.Credentials
import app.lentil.data.Finding
import app.lentil.data.Identity
import app.lentil.data.Intent
import app.lentil.data.IntentState
import app.lentil.data.IntentStore
import app.lentil.data.LedgerAccount
import app.lentil.data.LedgerError
import app.lentil.data.LedgerException
import app.lentil.data.LedgerRepository
import app.lentil.data.QueueEntry
import app.lentil.data.RuleWords
import app.lentil.data.SecretStore
import app.lentil.data.SettingsStore
import app.lentil.data.StoredConflict
import app.lentil.data.StoredIntent
import app.lentil.data.TagChoices
import java.time.Clock
import java.time.LocalDate
import java.util.UUID
import kotlin.time.Duration.Companion.milliseconds
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** The review queue and the sync status (PRD: FR-D1 to FR-D6). */
data class QueueUiState(
    val loading: Boolean = true,
    val items: List<QueueEntry> = emptyList(),
    val accounts: List<LedgerAccount> = emptyList(),
    val recentAccounts: List<String> = emptyList(),
    val defaultOpenDate: String? = null,
    /** Why the app may not write (spec section 13); empty if it may. */
    val readOnly: List<Finding> = emptyList(),
    /** Warnings of the last sync, such as failing balance assertions. */
    val warnings: List<Finding> = emptyList(),
    val pendingCount: Int = 0,
    val conflicts: List<StoredConflict> = emptyList(),
    val sync: SyncStatus = SyncStatus.Idle,
    val loadError: LedgerError? = null,
    /** The open tag picker, `null` while none is open. */
    val tagPicker: TagPicker? = null,
    /** The name of the running trip, `null` if none runs (PRD: FR-D13). */
    val runningTrip: String? = null,
    /** How many other open transactions the account sheet's rule would move; `null` until known. */
    val rulePreview: Int? = null,
    /** What the last account change moved, until its notice has shown. */
    val moved: Moved? = null,
) {
    val writable: Boolean
        get() = readOnly.isEmpty()

    /** Whether a trip may start or end: on a writable ledger, once a load told whether one runs. */
    val tripsEditable: Boolean
        get() = writable && !loading
}

/** The tag picker for [entry], with the [choices] the core computed for it (PRD: FR-D12). */
data class TagPicker(val entry: QueueEntry, val choices: TagChoices)

/**
 * The notice that one account change moved [count] other open transactions; [serial] tells two
 * changes with the same count apart, so each gets its notice.
 */
data class Moved(val count: Int, val serial: Long)

/** The last sync, in the user's terms rather than Git's (PRD: FR-D6). */
sealed interface SyncStatus {
    data object Idle : SyncStatus

    data object Syncing : SyncStatus

    data object UpToDate : SyncStatus

    data object Offline : SyncStatus

    data object SignInFailed : SyncStatus

    data object RemoteMissing : SyncStatus

    data object Busy : SyncStatus

    /** The changes would break the ledger; nothing was saved. */
    data class Blocked(val findings: List<Finding>) : SyncStatus

    /** The ledger needs repair by an admin before the app may write. */
    data class ReadOnly(val findings: List<Finding>) : SyncStatus

    data class Failed(val detail: String) : SyncStatus
}

/**
 * Triage on the local clone. Edits become pending intents in the app's storage and show at once
 * (PRD: FR-D5); a sync pushes them and turns conflicts into a visible list (PRD: FR-F4).
 */
@Suppress("TooManyFunctions")
class QueueViewModel(
    private val ledger: LedgerRepository,
    private val intents: IntentStore,
    private val settings: SettingsStore,
    private val secrets: SecretStore,
    /** `Lentil-Client` and `Lentil-Device`; `null` while the device has no ID. */
    private val identity: () -> Identity?,
    private val clock: Clock,
    private val newKey: () -> String = { UUID.randomUUID().toString() },
) : ViewModel() {
    private val mutableState = MutableStateFlow(QueueUiState())
    val state: StateFlow<QueueUiState> = mutableState.asStateFlow()

    private var intentState = IntentState()
    private val writing = Mutex()
    private var loadJob: Job? = null
    private var syncJob: Job? = null
    private var previewJob: Job? = null

    /** Takes corrections one at a time, so each is computed on the intents of those before it. */
    private val correcting = Mutex()

    /** The transactions whose correction the core is computing; a reload keeps them hidden. */
    private val inCorrection = mutableSetOf<String>()

    /** The serial of the last [Moved] notice. */
    private var movedSerial = 0L

    init {
        viewModelScope.launch {
            intentState = intents.read()
            publishIntents()
            reload()
            sync()
        }
    }

    fun confirm(entry: QueueEntry) {
        val id = editableId(entry) ?: return
        mutableState.update { state -> state.copy(items = state.items.without(id)) }
        record(entry.label, listOf(Intent.Confirm(id)))
    }

    fun dismiss(entry: QueueEntry) {
        val id = editableId(entry) ?: return
        mutableState.update { state -> state.copy(items = state.items.without(id)) }
        record(entry.label, listOf(Intent.Dismiss(id)))
    }

    /**
     * Sets the counter account of [entry] to the existing [account] and confirms it, because any
     * edit by the user reviews the transaction (spec section 6). With [rule], the words the user
     * picked, it also adds a rule that books like this (PRD: FR-D14).
     */
    fun changeAccount(entry: QueueEntry, account: String, rule: RuleWords? = null) {
        val change = accountChange(entry, account, rule) ?: return
        correct(change, null, rule)
    }

    /**
     * Opens [account] on the [openingDate], sets it as the counter account of [entry] and confirms
     * it; with [rule], also adds a rule that books like this.
     */
    fun openAccount(entry: QueueEntry, account: String, rule: RuleWords? = null) {
        val change = accountChange(entry, account, rule) ?: return
        val date = openingDate()
        mutableState.update { state ->
            state.copy(
                accounts = (state.accounts + LedgerAccount(account, date)).sortedBy { it.name }
            )
        }
        correct(change, date, rule)
    }

    /**
     * The date a new account opens on: that of the oldest open transaction, so it covers every one,
     * or today if there is none.
     */
    fun openingDate(): String =
        mutableState.value.defaultOpenDate ?: LocalDate.now(clock).toString()

    /**
     * Counts the other open transactions that [rule] would move if [entry] were corrected to
     * [account], opened on [openDate] if that is not `null`. Every tap and chip asks again, so the
     * core counts only once the choice has rested for [PREVIEW_DELAY], and only the latest count
     * lands.
     */
    fun previewRule(entry: QueueEntry, account: String, openDate: String?, rule: RuleWords) {
        val id = editableId(entry) ?: return
        previewJob?.cancel()
        mutableState.update { it.copy(rulePreview = null) }
        previewJob = viewModelScope.launch {
            delay(PREVIEW_DELAY)
            try {
                val count = ledger.rulePreview(intentState.pending, id, account, openDate, rule)
                mutableState.update { it.copy(rulePreview = count) }
            } catch (e: LedgerException) {
                mutableState.update { it.copy(loadError = e.error) }
            }
        }
    }

    /** Forgets [notice] once it has shown, unless a newer one replaced it. */
    fun clearMoved(notice: Moved) {
        mutableState.update { if (it.moved == notice) it.copy(moved = null) else it }
    }

    /** Removes [tag] from [entry]; tagging leaves the transaction open (spec section 6). */
    fun removeTag(entry: QueueEntry, tag: String) {
        val id = editableId(entry) ?: return
        if (shown(id)?.tags?.contains(tag) != true) return
        change(id) { it.copy(tags = it.tags - tag) }
        record(entry.label, listOf(Intent.RemoveTag(id, tag)))
    }

    /** Opens the tag picker for [entry] with the choices the core computes. */
    fun openTagPicker(entry: QueueEntry) {
        val id = editableId(entry) ?: return
        viewModelScope.launch {
            try {
                val choices = ledger.tagChoices(intentState.pending, id)
                mutableState.update { it.copy(tagPicker = TagPicker(entry, choices)) }
            } catch (e: LedgerException) {
                mutableState.update { it.copy(loadError = e.error) }
            }
        }
    }

    fun closeTagPicker() {
        mutableState.update { it.copy(tagPicker = null) }
    }

    /**
     * Adds [tag] to [entry] with the intents the core computes: it removes the tag's exclusive
     * rivals first. Tagging leaves the transaction open.
     */
    fun pickTag(entry: QueueEntry, tag: String) {
        val id = editableId(entry) ?: return
        closeTagPicker()
        viewModelScope.launch {
            try {
                record(entry.label, ledger.pickTag(intentState.pending, id, tag))
            } catch (e: LedgerException) {
                mutableState.update { it.copy(loadError = e.error) }
            }
        }
    }

    /** Accepts the trip tag suggested for [entry]; nothing else ever writes it (spec section 9). */
    fun acceptSuggestion(entry: QueueEntry) {
        val id = editableId(entry) ?: return
        val tag = shown(id)?.suggestedTag ?: return
        change(id) { it.copy(suggestedTag = null) }
        pickTag(entry, tag)
    }

    /** Whether [name] may name a trip or follow a tag family's prefix; the core's rule. */
    fun isPlainName(name: String): Boolean = ledger.isPlainName(name)

    /**
     * Starts the trip [name] today, as an event with a new ID of this device, so it works offline
     * and shows as running at once (spec section 9).
     */
    fun startTrip(name: String) {
        val device = tripDevice() ?: return
        if (!ledger.isPlainName(name)) return
        val today = LocalDate.now(clock).toString()
        mutableState.update { it.copy(runningTrip = name) }
        record(today, listOf(Intent.StartTrip(ledger.newManualId(device), today, name)))
    }

    /** Ends the running trip today. */
    fun endTrip() {
        val device = tripDevice() ?: return
        if (mutableState.value.runningTrip == null) return
        val today = LocalDate.now(clock).toString()
        mutableState.update { it.copy(runningTrip = null) }
        record(today, listOf(Intent.EndTrip(ledger.newManualId(device), today)))
    }

    fun discardConflict(key: String) {
        intentState = intentState.discard(key)
        publishIntents()
        viewModelScope.launch { persist() }
    }

    /** Syncs now: on start, on request, and when the app goes to the background. */
    fun sync() {
        if (syncJob?.isActive == true) return
        syncJob = viewModelScope.launch { runSync() }
    }

    /** Syncs if anything waits, for the end of a session (spec section 11). */
    fun syncIfPending() {
        if (intentState.pending.isNotEmpty()) sync()
    }

    private suspend fun runSync() {
        val connection = settings.connection
        val token = secrets.token()
        val identity = identity()
        val status =
            when {
                connection == null || token == null -> SyncStatus.SignInFailed
                identity == null -> SyncStatus.Failed("no device ID")
                else -> {
                    mutableState.update { it.copy(sync = SyncStatus.Syncing) }
                    try {
                        val result =
                            ledger.sync(
                                intentState.pending,
                                identity,
                                Author(connection.authorName, connection.authorEmail),
                                Credentials(connection.username, token),
                            )
                        intentState = intentState.afterSync(result.outcomes)
                        mutableState.update { it.copy(warnings = result.warnings) }
                        SyncStatus.UpToDate
                    } catch (e: LedgerException) {
                        e.error.toSyncStatus()
                    }
                }
            }
        mutableState.update { it.copy(sync = status) }
        publishIntents()
        persist()
        reload()
    }

    /** The device that writes trip events; `null` while the app may not write or has no ID. */
    private fun tripDevice(): String? =
        identity()?.device?.takeIf { mutableState.value.tripsEditable }

    private fun editableId(entry: QueueEntry): String? =
        entry.id.takeIf { mutableState.value.writable }

    /** The transaction [id] as the queue shows it now; `null` once it left the queue. */
    private fun shown(id: String): QueueEntry? = mutableState.value.items.find { it.id == id }

    /** Shows [edit] of the transaction [id] at once; the next reload shows the core's result. */
    private fun change(id: String, edit: (QueueEntry) -> QueueEntry) {
        mutableState.update { state ->
            state.copy(items = state.items.map { if (it.id == id) edit(it) else it })
        }
    }

    /**
     * Hides [entry] and remembers [account] if the user may change its counter account to
     * [account]; returns the change then, otherwise `null`. Its current account changes nothing,
     * unless the user makes a [rule] of it.
     */
    private fun accountChange(
        entry: QueueEntry,
        account: String,
        rule: RuleWords?,
    ): AccountChange? {
        val id = editableId(entry) ?: return null
        val posting = entry.counterPosting ?: return null
        val old = entry.counterAccount ?: return null
        if (account == old && rule == null) return null
        settings.rememberAccount(account)
        mutableState.update { state ->
            state.copy(items = state.items.without(id), recentAccounts = settings.recentAccounts())
        }
        return AccountChange(entry, id, posting, old, account)
    }

    /**
     * Records the intents the core computes for [change], with the account opened on [openDate] if
     * that is not `null`: the user's own, the rule of [rule], and the suggestions that move other
     * open transactions with it (PRD: FR-D14, FR-D15).
     *
     * The user's own change is never lost: it runs to its end and is stored even if the view model
     * is cleared meanwhile, and corrections run one at a time.
     */
    private fun correct(change: AccountChange, openDate: String?, rule: RuleWords?) {
        inCorrection += change.id
        // The rows as the user saw them, before reloads replace them.
        val labels =
            mutableState.value.items.mapNotNull { row -> row.id?.let { it to row.label } }.toMap()
        viewModelScope.launch(start = CoroutineStart.UNDISPATCHED) {
            withContext(NonCancellable) {
                correcting.withLock { recordCorrection(change, openDate, rule, labels) }
            }
        }
    }

    /**
     * Computes and stores the intents of [change]. Each suggestion carries the label of its own
     * transaction in [labels], so a conflict names the transaction that conflicted. If the core
     * fails, the user's own change is stored without the rule and the error shows.
     */
    private suspend fun recordCorrection(
        change: AccountChange,
        openDate: String?,
        rule: RuleWords?,
        labels: Map<String, String>,
    ) {
        val label = change.entry.label
        val failure =
            try {
                val pending = intentState.pending
                val correction = ledger.correct(pending, change.id, change.account, openDate, rule)
                add(
                    correction.intents.map { intent ->
                        val own = (intent as? Intent.SuggestAccount)?.let { labels[it.id] }
                        StoredIntent(newKey(), intent, own ?: label)
                    }
                )
                if (correction.moved > 0) {
                    val notice = Moved(correction.moved, ++movedSerial)
                    mutableState.update { it.copy(moved = notice) }
                }
                null
            } catch (e: LedgerException) {
                add(change.plain(openDate).map { StoredIntent(newKey(), it, label) })
                e.error
            } finally {
                inCorrection -= change.id
            }
        persist()
        failure?.let { error ->
            // After the reload, which would clear it.
            loadJob?.join()
            mutableState.update { it.copy(loadError = error) }
        }
    }

    private fun record(label: String, new: List<Intent>) {
        add(new.map { StoredIntent(newKey(), it, label) })
        viewModelScope.launch { persist() }
    }

    /** Adds [new] to the pending intents and shows them at once; the caller persists them. */
    private fun add(new: List<StoredIntent>) {
        intentState = intentState.add(new)
        publishIntents()
        // At once: a load that started before these intents would undo what the screen shows.
        reload()
    }

    private fun publishIntents() {
        mutableState.update {
            it.copy(
                pendingCount = intentState.pending.size,
                conflicts = intentState.conflicts,
                recentAccounts = settings.recentAccounts(),
            )
        }
    }

    /** Writes the newest state; the lock keeps an older write from landing last. */
    private suspend fun persist() {
        writing.withLock { intents.write(intentState) }
    }

    private fun reload() {
        loadJob?.cancel()
        loadJob = viewModelScope.launch {
            val pending = intentState.pending
            try {
                val view = ledger.load(pending)
                val queue = view.queue.filterNot { it.id in inCorrection }
                mutableState.update {
                    it.copy(
                        loading = false,
                        items = queue,
                        accounts = view.accounts,
                        defaultOpenDate = view.defaultOpenDate,
                        readOnly = view.readOnly,
                        runningTrip = view.runningTrip,
                        loadError = null,
                        tagPicker = it.tagPicker?.within(queue, view.readOnly.isEmpty()),
                    )
                }
            } catch (e: LedgerException) {
                val error = e.error
                mutableState.update {
                    it.copy(
                        loading = false,
                        items = emptyList(),
                        readOnly = (error as? LedgerError.ReadOnly)?.findings.orEmpty(),
                        loadError = error.takeUnless { error is LedgerError.ReadOnly },
                        tagPicker = null,
                    )
                }
            }
        }
    }
}

/** How long a rule's choice must rest before the core counts what it would move. */
private val PREVIEW_DELAY = 300.milliseconds

/** The change of [entry]'s counter posting [posting], with the ID [id], from [old] to [account]. */
private data class AccountChange(
    val entry: QueueEntry,
    val id: String,
    val posting: Int,
    val old: String,
    val account: String,
) {
    /**
     * The user's own intents without the core: the account opened on [openDate] if that is not
     * `null`, the change and its confirmation.
     */
    fun plain(openDate: String?): List<Intent> =
        listOfNotNull(
            openDate?.let { Intent.OpenAccount(account, it) },
            Intent.SetAccount(id, posting, old, account),
            Intent.Confirm(id),
        )
}

/**
 * The queue without the transaction [id]. It matches by ID, because a reload may have changed the
 * transaction's other fields, such as its tags, since the screen showed it.
 */
private fun List<QueueEntry>.without(id: String): List<QueueEntry> = filterNot { it.id == id }

/** The picker for its transaction as [queue] shows it now; `null` once it left or edits stopped. */
private fun TagPicker.within(queue: List<QueueEntry>, writable: Boolean): TagPicker? =
    queue.find { it.id == entry.id }?.takeIf { writable }?.let { copy(entry = it) }

private fun LedgerError.toSyncStatus(): SyncStatus =
    when (this) {
        LedgerError.Network -> SyncStatus.Offline
        LedgerError.Auth -> SyncStatus.SignInFailed
        LedgerError.NotFound -> SyncStatus.RemoteMissing
        LedgerError.TooManyAttempts -> SyncStatus.Busy
        is LedgerError.Blocked -> SyncStatus.Blocked(findings)
        is LedgerError.ReadOnly -> SyncStatus.ReadOnly(findings)
        LedgerError.LocalChanges -> SyncStatus.Failed("local changes")
        LedgerError.NotCorrectable -> SyncStatus.Failed("transaction changed")
        is LedgerError.Io -> SyncStatus.Failed(detail)
        is LedgerError.Other -> SyncStatus.Failed(detail)
    }
