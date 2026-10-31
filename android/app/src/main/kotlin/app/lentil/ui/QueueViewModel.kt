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
import app.lentil.data.LedgerError
import app.lentil.data.LedgerException
import app.lentil.data.LedgerRepository
import app.lentil.data.QueueEntry
import app.lentil.data.SecretStore
import app.lentil.data.SettingsStore
import app.lentil.data.StoredConflict
import app.lentil.data.StoredIntent
import java.time.Clock
import java.time.LocalDate
import java.util.UUID
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** The review queue and the sync status (PRD: FR-D1 to FR-D6). */
data class QueueUiState(
    val loading: Boolean = true,
    val items: List<QueueEntry> = emptyList(),
    val accounts: List<String> = emptyList(),
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
) {
    val writable: Boolean
        get() = readOnly.isEmpty()
}

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
        mutableState.update { state -> state.copy(items = state.items - entry) }
        record(entry, Intent.Confirm(id))
    }

    fun dismiss(entry: QueueEntry) {
        val id = editableId(entry) ?: return
        mutableState.update { state -> state.copy(items = state.items - entry) }
        record(entry, Intent.Dismiss(id))
    }

    /** Sets the counter account of [entry] to the existing [account]. */
    fun changeAccount(entry: QueueEntry, account: String) {
        val change = accountChange(entry, account) ?: return
        record(entry, change)
    }

    /**
     * Opens [account], dated like the oldest transaction so it covers every open one, and sets it
     * as the counter account of [entry].
     */
    fun openAccount(entry: QueueEntry, account: String) {
        val change = accountChange(entry, account) ?: return
        val date = mutableState.value.defaultOpenDate ?: LocalDate.now(clock).toString()
        mutableState.update { state -> state.copy(accounts = (state.accounts + account).sorted()) }
        record(entry, Intent.OpenAccount(account, date), change)
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

    private fun editableId(entry: QueueEntry): String? =
        entry.id.takeIf { mutableState.value.writable }

    private fun accountChange(entry: QueueEntry, account: String): Intent.SetAccount? {
        val id = editableId(entry) ?: return null
        val posting = entry.counterPosting
        val old = entry.counterAccount
        if (posting == null || old == null || account == old) return null
        settings.rememberAccount(account)
        mutableState.update { state ->
            state.copy(
                items =
                    state.items.map { if (it == entry) it.copy(counterAccount = account) else it },
                recentAccounts = settings.recentAccounts(),
            )
        }
        return Intent.SetAccount(id = id, posting = posting, old = old, new = account)
    }

    private fun record(entry: QueueEntry, vararg new: Intent) {
        intentState = intentState.add(new.map { StoredIntent(newKey(), it, entry.label) })
        publishIntents()
        viewModelScope.launch {
            persist()
            reload()
        }
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
                mutableState.update {
                    it.copy(
                        loading = false,
                        items = view.queue,
                        accounts = view.accounts,
                        defaultOpenDate = view.defaultOpenDate,
                        readOnly = view.readOnly,
                        loadError = null,
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
                    )
                }
            }
        }
    }
}

private fun LedgerError.toSyncStatus(): SyncStatus =
    when (this) {
        LedgerError.Network -> SyncStatus.Offline
        LedgerError.Auth -> SyncStatus.SignInFailed
        LedgerError.NotFound -> SyncStatus.RemoteMissing
        LedgerError.TooManyAttempts -> SyncStatus.Busy
        is LedgerError.Blocked -> SyncStatus.Blocked(findings)
        is LedgerError.ReadOnly -> SyncStatus.ReadOnly(findings)
        LedgerError.LocalChanges -> SyncStatus.Failed("local changes")
        is LedgerError.Io -> SyncStatus.Failed(detail)
        is LedgerError.Other -> SyncStatus.Failed(detail)
    }
