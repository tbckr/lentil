package app.lentil.ui

import app.lentil.data.Author
import app.lentil.data.Connection
import app.lentil.data.Credentials
import app.lentil.data.Identity
import app.lentil.data.Intent
import app.lentil.data.IntentOutcome
import app.lentil.data.IntentState
import app.lentil.data.IntentStore
import app.lentil.data.LedgerAccount
import app.lentil.data.LedgerError
import app.lentil.data.LedgerException
import app.lentil.data.LedgerRepository
import app.lentil.data.LedgerView
import app.lentil.data.QueueEntry
import app.lentil.data.SecretStore
import app.lentil.data.SettingsStore
import app.lentil.data.StoredIntent
import app.lentil.data.TagChoices
import app.lentil.data.TagSource
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.rules.TestWatcher
import org.junit.runner.Description

/** Runs `viewModelScope` on a test dispatcher. */
@OptIn(ExperimentalCoroutinesApi::class)
class MainDispatcherRule(val dispatcher: TestDispatcher = StandardTestDispatcher()) :
    TestWatcher() {
    override fun starting(description: Description) = Dispatchers.setMain(dispatcher)

    override fun finished(description: Description) = Dispatchers.resetMain()
}

fun entry(
    id: String?,
    date: String = "2026-10-18",
    counter: String? = "Expenses:Uncategorized",
    posting: Int? = 1,
    tags: List<String> = emptyList(),
    suggested: String? = null,
) =
    QueueEntry(
        id = id,
        date = date,
        payee = "Corner Grocery",
        narration = "Weekly shopping",
        desc = "CORNER GROCERY 4711 CARD PAYMENT",
        amount = "-42.10",
        currency = "EUR",
        sourceAccount = "Assets:Bank:Giro",
        counterAccount = counter,
        counterPosting = posting,
        file = "imports/giro/2026/20261020T071502Z.beancount",
        line = 1,
        tags = tags,
        suggestedTag = suggested,
    )

/** A ledger whose queue hides confirmed and dismissed transactions, like the core's replay. */
class FakeLedger(
    var queue: List<QueueEntry> = emptyList(),
    var accounts: List<LedgerAccount> =
        listOf(
            LedgerAccount("Expenses:Groceries", "2026-01-01"),
            LedgerAccount("Expenses:Uncategorized", "2026-01-01"),
        ),
    var defaultOpenDate: String? = "2026-10-18",
) : LedgerRepository {
    var readOnly = emptyList<app.lentil.data.Finding>()
    var loadError: LedgerError? = null
    var syncError: LedgerError? = null
    var cloneError: LedgerError? = null
    var outcomes: (List<StoredIntent>) -> Map<String, IntentOutcome> = { pending ->
        pending.associate { it.key to IntentOutcome.Applied }
    }
    var warnings = emptyList<app.lentil.data.Finding>()
    var runningTrip: String? = null
    var choices = TagChoices(emptyList(), emptyList(), TagSource.Taxonomy)
    var picks: (String, String) -> List<Intent> = { id, tag -> listOf(Intent.AddTag(id, tag)) }
    val picked = mutableListOf<Pair<String, String>>()

    /** If set, a sync waits for it, so a test can act while the sync runs. */
    var syncGate: CompletableDeferred<Unit>? = null

    /** If set, a load waits for it and then returns the ledger as it was when the load started. */
    var loadGate: CompletableDeferred<Unit>? = null

    val loads = mutableListOf<List<StoredIntent>>()
    val syncs = mutableListOf<SyncCall>()
    var cloned: Pair<String, Credentials>? = null

    data class SyncCall(
        val pending: List<StoredIntent>,
        val identity: Identity,
        val author: Author,
        val username: String,
        val token: String,
    )

    override fun hasClone(): Boolean = cloned != null

    override suspend fun clone(url: String, credentials: Credentials) {
        cloneError?.let { throw LedgerException(it) }
        cloned = url to credentials
    }

    override suspend fun load(pending: List<StoredIntent>): LedgerView {
        loads += pending
        loadError?.let { throw LedgerException(it) }
        val done =
            pending
                .mapNotNull {
                    when (val intent = it.intent) {
                        is Intent.Confirm -> intent.id
                        is Intent.Dismiss -> intent.id
                        else -> null
                    }
                }
                .toSet()
        val view =
            LedgerView(
                queue = queue.filterNot { it.id in done },
                accounts = accounts,
                defaultOpenDate = defaultOpenDate,
                readOnly = readOnly,
                runningTrip = runningTrip,
            )
        loadGate?.await()
        return view
    }

    override suspend fun tagChoices(pending: List<StoredIntent>, id: String): TagChoices {
        loadError?.let { throw LedgerException(it) }
        return choices
    }

    override suspend fun pickTag(
        pending: List<StoredIntent>,
        id: String,
        tag: String,
    ): List<Intent> {
        loadError?.let { throw LedgerException(it) }
        picked += id to tag
        return picks(id, tag)
    }

    override fun newManualId(device: String): String = "$device-01jc8m2w9z4f7q3r5t6v8x0y1a"

    /** The core's rule, `[a-z0-9-]+`, standing in for it in tests. */
    override fun isPlainName(name: String): Boolean = PLAIN.matches(name)

    override suspend fun sync(
        pending: List<StoredIntent>,
        identity: Identity,
        author: Author,
        credentials: Credentials,
    ): app.lentil.data.SyncResult {
        syncs += SyncCall(pending, identity, author, credentials.username, credentials.token)
        syncGate?.await()
        syncError?.let { throw LedgerException(it) }
        return app.lentil.data.SyncResult(outcomes(pending), committed = true, warnings = warnings)
    }

    override fun newDeviceId(): String = "k7p2q9xd"
}

private val PLAIN = Regex("[a-z0-9-]+")

class FakeIntentStore(var state: IntentState = IntentState()) : IntentStore {
    var writes = 0

    override suspend fun read(): IntentState = state

    override suspend fun write(state: IntentState) {
        this.state = state
        writes++
    }
}

class FakeSettings(
    override var deviceId: String? = "k7p2q9xd",
    override var connection: Connection? =
        Connection(
            url = "https://git.example.org/alex/ledger.git",
            username = "alex",
            authorName = "Alex Example",
            authorEmail = "alex@example.org",
        ),
) : SettingsStore {
    private val recent = mutableListOf<String>()

    override fun recentAccounts(): List<String> = recent.toList()

    override fun rememberAccount(account: String) {
        recent.remove(account)
        recent.add(0, account)
    }
}

class FakeSecrets(private var stored: String? = "secret-token") : SecretStore {
    override fun hasToken(): Boolean = stored != null

    override fun token(): String? = stored

    override fun saveToken(token: String) {
        stored = token
    }

    override fun clear() {
        stored = null
    }
}
