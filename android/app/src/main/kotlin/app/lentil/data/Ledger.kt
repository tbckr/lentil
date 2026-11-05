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
    /** Its tags without `#`, in file order. */
    val tags: List<String> = emptyList(),
    /** The tag of the trip its date lies in, until the user accepts it (spec section 9). */
    val suggestedTag: String? = null,
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
    /** The name of the running trip, `null` if none runs. */
    val runningTrip: String? = null,
)

/** Where the tag picker's choices come from; the picker shows a hint unless it is [Taxonomy]. */
enum class TagSource {
    /** From `tags.toml`. */
    Taxonomy,
    /** There is no `tags.toml`: the tags the ledger uses. */
    Missing,
    /** `tags.toml` doesn't parse: the tags the ledger uses. */
    Invalid,
}

/** A tag the picker offers, with the transaction's tags that picking it replaces. */
data class TagChoice(val tag: String, val replaces: List<String>)

/** What the tag picker offers for one transaction, as the core computes it. */
data class TagChoices(
    val tags: List<TagChoice>,
    /** Prefixes such as `share-`: the user types the rest. */
    val families: List<String>,
    val source: TagSource,
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

    /** What the tag picker offers for transaction [id] with [pending] applied. */
    suspend fun tagChoices(pending: List<StoredIntent>, id: String): TagChoices

    /** The intents of picking [tag] for transaction [id]: removing its rivals, then adding it. */
    suspend fun pickTag(pending: List<StoredIntent>, id: String, tag: String): List<Intent>

    /** A new `lentil_id` for an entry this device creates, such as a trip event. */
    fun newManualId(device: String): String

    /** Whether [name] is a valid trip name and a valid rest after a tag family's prefix. */
    fun isPlainName(name: String): Boolean
}
