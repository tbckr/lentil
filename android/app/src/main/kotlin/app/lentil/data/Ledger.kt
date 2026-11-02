package app.lentil.data

/** One open transaction of the review queue. */
data class QueueEntry(
    /** `lentil_id`; without one the transaction is read-only. */
    val id: String?,
    val date: String,
    val payee: String?,
    val narration: String,
    /** The bank's original text: counterparty and purpose. */
    val desc: String?,
    val amount: String?,
    val currency: String?,
    val sourceAccount: String?,
    /** The suggested counter account; `null` for splits, which stay read-only. */
    val counterAccount: String?,
    val counterPosting: Int?,
    val file: String,
    val line: Int,
) {
    /** Whether the app can edit the transaction at all. */
    val editable: Boolean
        get() = id != null

    /** A short description for conflict lists. */
    val label: String
        get() = listOfNotNull(date, payee, narration.ifBlank { null }).joinToString(" · ")
}

/** A validation finding of the core; the app translates [code] and inserts [args]. */
data class Finding(
    val code: String,
    val blocking: Boolean,
    val file: String?,
    val line: Int?,
    val args: Map<String, String>,
)

/** An opened account of the ledger. */
data class LedgerAccount(
    val name: String,
    /**
     * Its open date as `YYYY-MM-DD`: a counter posting of a transaction on or after it may move to
     * the account. `null` if none may, because the account is closed or an imported account.
     */
    val openFrom: String?,
)

/** The names of the accounts a counter posting of a transaction on [date] may move to. */
fun List<LedgerAccount>.usableOn(date: String): List<String> =
    filter { account -> account.openFrom?.let { it <= date } == true }.map { it.name }

/** Everything the queue screen shows of the local ledger. */
data class LedgerView(
    val queue: List<QueueEntry>,
    val accounts: List<LedgerAccount>,
    val defaultOpenDate: String?,
    /** Why the app may not write; empty if it may. */
    val readOnly: List<Finding>,
)

/** What a sync did with one intent. */
sealed interface IntentOutcome {
    data object Applied : IntentOutcome

    data object AlreadyApplied : IntentOutcome

    data class Conflict(val code: String, val current: String?) : IntentOutcome
}

/** The result of a successful sync. */
data class SyncResult(
    val outcomes: Map<String, IntentOutcome>,
    val committed: Boolean,
    val warnings: List<Finding>,
)

/** Remote credentials: HTTPS user name and token. */
class Credentials(val username: String, val token: String) {
    override fun toString(): String = "Credentials(username=$username, token=***)"
}

/** The values of the `Lentil-Client` and `Lentil-Device` trailers. */
data class Identity(val client: String, val device: String)

/** Commit author from the app's settings. */
data class Author(val name: String, val email: String)

/** Why a ledger operation failed, as codes the UI translates. */
sealed interface LedgerError {
    data object Network : LedgerError

    data object Auth : LedgerError

    data object NotFound : LedgerError

    data object LocalChanges : LedgerError

    data class ReadOnly(val findings: List<Finding>) : LedgerError

    data class Blocked(val findings: List<Finding>) : LedgerError

    data object TooManyAttempts : LedgerError

    data class Io(val detail: String) : LedgerError

    data class Other(val detail: String) : LedgerError
}

/** Thrown by [LedgerRepository] with a typed [error]. */
class LedgerException(val error: LedgerError, cause: Throwable? = null) :
    Exception(error.toString(), cause)

/** The Git clone and the core behind it: the app's I/O boundary to the ledger. */
interface LedgerRepository {
    /** Whether a clone exists on the device. */
    fun hasClone(): Boolean

    /** Clones [url]; throws [LedgerException]. */
    suspend fun clone(url: String, credentials: Credentials)

    /** The local ledger with [pending] applied; throws [LedgerException]. */
    suspend fun load(pending: List<StoredIntent>): LedgerView

    /** Pushes [pending]; throws [LedgerException]. */
    suspend fun sync(
        pending: List<StoredIntent>,
        identity: Identity,
        author: Author,
        credentials: Credentials,
    ): SyncResult

    /** A new device ID (spec section 9). */
    fun newDeviceId(): String
}
